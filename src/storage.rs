use crate::{
    config::{Config, Locations},
    model::{now, ProviderStatus, Snapshot, SCHEMA_VERSION},
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

pub fn private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|_| "Cannot create private data directory")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot set directory permissions".into())
}
pub struct WriterLock(File);
impl Drop for WriterLock {
    fn drop(&mut self) {
        // Explicit unlock also releases a briefly fork-inherited open description.
        let _ = self.0.unlock();
    }
}
pub fn existing_writer_lock(loc: &Locations) -> Result<Option<WriterLock>, String> {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(loc.state.join("writer.lock"))
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot inspect existing writer lock".into()),
    };
    file.try_lock()
        .map_err(|_| "Another update is in progress; retry shortly".to_string())?;
    Ok(Some(WriterLock(file)))
}
pub fn writer_lock(loc: &Locations) -> Result<WriterLock, String> {
    private_dir(&loc.state)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(loc.state.join("writer.lock"))
        .map_err(|_| "Cannot open writer lock")?;
    file.try_lock()
        .map_err(|_| "Another update is in progress; retry shortly".to_string())?;
    Ok(WriterLock(file))
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing cache parent")?;
    private_dir(parent)?;
    let tmp = parent.join(format!(
        ".status-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|_| "Cannot create cache temporary file")?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "Cannot flush cache")?;
        fs::rename(&tmp, path).map_err(|_| "Cannot replace cache")?;
        File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(|_| "Cannot sync cache directory")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
pub fn read_cache(path: &Path) -> Result<Snapshot, String> {
    let data = fs::read(path).map_err(|_| "No readable cache; run ai-quota refresh")?;
    if data.len() > 4_194_304 {
        return Err("Cache too large".into());
    }
    let snapshot: Snapshot = serde_json::from_slice(&data).map_err(|_| "Invalid cache JSON")?;
    snapshot.validate()?;
    Ok(snapshot)
}
pub fn empty(config: &Config) -> Snapshot {
    Snapshot {
        schema_version: SCHEMA_VERSION,
        generated_at: now(),
        stale_seconds: config.stale_seconds,
        providers: config
            .providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| ProviderStatus::empty(&p.provider, &p.profile))
            .collect(),
    }
}
pub fn open(loc: &Locations) -> Result<Connection, String> {
    private_dir(&loc.state)?;
    let path = loc.state.join("usage.sqlite3");
    let db = Connection::open(&path).map_err(|_| "Cannot open database")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|_| "Cannot set database permissions")?;
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "Database timeout setup failed")?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS state (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS samples (id INTEGER PRIMARY KEY, provider TEXT NOT NULL, profile TEXT NOT NULL, sampled_at INTEGER NOT NULL, window TEXT NOT NULL, label TEXT NOT NULL, duration_minutes INTEGER, resets_at INTEGER, used_percent REAL NOT NULL, remaining_percent REAL NOT NULL);
        CREATE INDEX IF NOT EXISTS sample_time ON samples(sampled_at);
        CREATE TABLE IF NOT EXISTS alerts (provider TEXT NOT NULL, profile TEXT NOT NULL, window TEXT NOT NULL, reset_at INTEGER, level INTEGER NOT NULL, PRIMARY KEY(provider, profile, window));
        CREATE TABLE IF NOT EXISTS imports (source TEXT PRIMARY KEY);") .map_err(|_| "Database schema setup failed")?;
    Ok(db)
}
pub fn durable(db: &Connection, config: &Config) -> Result<Snapshot, String> {
    let json: Option<String> = db
        .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
        .optional()
        .map_err(|_| "Cannot read durable state")?;
    let mut snapshot = match json {
        Some(json) => {
            serde_json::from_str::<Snapshot>(&json).map_err(|_| "Invalid durable state")?
        }
        None => empty(config),
    };
    snapshot.validate()?;
    snapshot.stale_seconds = config.stale_seconds;
    snapshot.providers.retain(|p| {
        config
            .providers
            .iter()
            .any(|c| c.enabled && c.provider == p.provider && c.profile == p.profile)
    });
    for c in config.providers.iter().filter(|c| c.enabled) {
        if !snapshot
            .providers
            .iter()
            .any(|p| p.provider == c.provider && p.profile == c.profile)
        {
            snapshot
                .providers
                .push(ProviderStatus::empty(&c.provider, &c.profile));
        }
    }
    Ok(snapshot)
}
pub struct Alert {
    pub message: String,
    pub threshold: u8,
}
pub fn save(
    db: &mut Connection,
    loc: &Locations,
    snapshot: &Snapshot,
    successful: &[(String, String)],
    config: &Config,
) -> Result<Vec<Alert>, String> {
    snapshot.validate()?;
    let tx = db.transaction().map_err(|_| "Cannot begin transaction")?;
    let mut notifications = vec![];
    for p in snapshot
        .providers
        .iter()
        .filter(|p| successful.contains(&(p.provider.clone(), p.profile.clone())))
    {
        for q in &p.quotas {
            tx.execute("INSERT INTO samples(provider,profile,sampled_at,window,label,duration_minutes,resets_at,used_percent,remaining_percent) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![p.provider,p.profile,p.last_success_at,q.window,q.label,q.duration_minutes,q.resets_at,q.used_percent,q.remaining_percent]).map_err(|_| "Cannot save sample")?;
            let previous: Option<(Option<i64>, u8)> = tx.query_row("SELECT reset_at,level FROM alerts WHERE provider=?1 AND profile=?2 AND window=?3", params![p.provider,p.profile,q.window], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_| "Cannot read alert state")?;
            let level = if q.remaining_percent <= 5.0 {
                5
            } else if q.remaining_percent <= 10.0 {
                10
            } else if q.remaining_percent <= 20.0 {
                20
            } else {
                100
            };
            let new_period =
                previous
                    .as_ref()
                    .is_some_and(|(reset, _)| match (*reset, q.resets_at) {
                        (Some(a), Some(b)) => (a - b).abs() > 300,
                        _ => false,
                    });
            let old = previous.as_ref().map(|(_, level)| *level).unwrap_or(100);
            if level < 100 && (new_period || level < old) {
                notifications.push(Alert {
                    threshold: level,
                    message: format!(
                        "{} [{}] {}: {:.1}% remaining",
                        p.provider, p.profile, q.label, q.remaining_percent
                    ),
                });
            }
            // With unknown reset, re-arm only after observed recovery above the threshold.
            let saved_level = if !new_period && q.resets_at.is_some() {
                old.min(level)
            } else {
                level
            };
            let reset_id = if !new_period {
                previous
                    .as_ref()
                    .and_then(|(reset, _)| *reset)
                    .or(q.resets_at)
            } else {
                q.resets_at
            };
            tx.execute("INSERT INTO alerts(provider,profile,window,reset_at,level) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(provider,profile,window) DO UPDATE SET reset_at=excluded.reset_at,level=excluded.level",params![p.provider,p.profile,q.window,reset_id,saved_level]).map_err(|_| "Cannot save alert state")?;
        }
    }
    let bytes = serde_json::to_vec(snapshot).map_err(|_| "Cannot serialize state")?;
    tx.execute(
        "INSERT INTO state(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
        [std::str::from_utf8(&bytes).map_err(|_| "Invalid serialization")?],
    )
    .map_err(|_| "Cannot save state")?;
    if let Some(days) = config.retention_days {
        tx.execute(
            "DELETE FROM samples WHERE sampled_at < ?1",
            [snapshot.generated_at - i64::from(days) * 86400],
        )
        .map_err(|_| "Cannot apply retention")?;
    }
    tx.commit().map_err(|_| "Cannot commit state")?;
    atomic_write(&loc.cache, &bytes)?;
    Ok(notifications)
}
pub fn history(
    loc: &Locations,
    days: u32,
    csv: bool,
    provider: Option<&str>,
) -> Result<String, String> {
    let db = Connection::open_with_flags(
        loc.state.join("usage.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "No readable history")?;
    let mut stmt = db.prepare("SELECT sampled_at,provider,profile,window,label,duration_minutes,resets_at,used_percent,remaining_percent FROM samples WHERE sampled_at>=?1 AND (?2 IS NULL OR provider=?2) ORDER BY sampled_at DESC,id DESC").map_err(|_| "Cannot query history")?;
    let rows = stmt
        .query_map(params![now() - i64::from(days) * 86400, provider], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<u64>>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, f64>(7)?,
                r.get::<_, f64>(8)?,
            ))
        })
        .map_err(|_| "Cannot read history")?;
    let mut out = if csv {
        "sampled_at,provider,profile,window,label,duration_minutes,resets_at,used_percent,remaining_percent\n".into()
    } else {
        String::new()
    };
    let escape = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    for (i, row) in rows.enumerate() {
        if !csv && i >= 30 {
            out.push_str("… use export for all samples\n");
            break;
        }
        let (at, p, profile, w, label, mins, reset, used, left) =
            row.map_err(|_| "Invalid history row")?;
        if csv {
            out.push_str(&format!(
                "{at},{},{},{},{},{},{},{used},{left}\n",
                escape(&p),
                escape(&profile),
                escape(&w),
                escape(&label),
                mins.map(|n| n.to_string()).unwrap_or_default(),
                reset.map(|n| n.to_string()).unwrap_or_default()
            ));
        } else {
            out.push_str(&format!(
                "{at} | {p} [{profile}] {label} | {left:.1}% remaining | reset {}\n",
                reset
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
    }
    Ok(out)
}

pub fn migrate(
    config: &Config,
    loc: &Locations,
    source: &Path,
    provider: &str,
) -> Result<(), String> {
    let _lock = writer_lock(loc)?;
    let original = if source.is_dir() {
        source.join("usage.sqlite3")
    } else {
        source.to_path_buf()
    };
    let identity = format!(
        "{}:{provider}",
        original
            .canonicalize()
            .map_err(|_| "Missing legacy database")?
            .display()
    );
    let mut db = open(loc)?;
    if db
        .query_row(
            "SELECT source FROM imports WHERE source=?1",
            [&identity],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| "Cannot inspect imports")?
        .is_some()
    {
        return Ok(());
    }
    // Copy database and WAL into our private directory. SQLite never opens the original.
    let temporary = loc
        .state
        .join(format!("migration-{}-{}", std::process::id(), now()));
    private_dir(&temporary)?;
    let result = (|| {
        let wal = std::path::PathBuf::from(format!("{}-wal", original.display()));
        let signature = || -> Result<_, String> {
            Ok((
                fs::metadata(&original)
                    .map_err(|_| "Cannot inspect source")?
                    .modified()
                    .map_err(|_| "Cannot inspect timestamp")?,
                fs::metadata(&original)
                    .map_err(|_| "Cannot inspect source")?
                    .len(),
                fs::metadata(&wal)
                    .ok()
                    .map(|m| (m.len(), m.modified().ok())),
            ))
        };
        let before = signature()?;
        let copy = temporary.join("usage.sqlite3");
        fs::copy(&original, &copy).map_err(|_| "Cannot copy legacy database")?;
        if wal.exists() {
            fs::copy(&wal, temporary.join("usage.sqlite3-wal"))
                .map_err(|_| "Cannot copy legacy WAL")?;
        }
        if signature()? != before {
            return Err("Source changed while copying; retry migration".into());
        }
        let legacy = Connection::open(&copy).map_err(|_| "Cannot open isolated legacy copy")?;
        let integrity: String = legacy
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .map_err(|_| "Legacy integrity check failed")?;
        if integrity != "ok" {
            return Err("Legacy snapshot inconsistent; retry migration".into());
        }
        let tx = db.transaction().map_err(|_| "Cannot begin import")?;
        let mut stmt=legacy.prepare("SELECT sampled_at,window_kind,duration_minutes,resets_at,used_percent,remaining_percent FROM samples WHERE bucket_id=?1 ORDER BY id").map_err(|_|"Unsupported legacy schema")?;
        let rows = stmt
            .query_map([provider], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, f64>(5)?,
                ))
            })
            .map_err(|_| "Cannot read legacy samples")?;
        let mut latest: std::collections::BTreeMap<String, (i64, crate::model::Quota)> =
            std::collections::BTreeMap::new();
        for row in rows {
            let (at, window, mins, reset, used, left) = row.map_err(|_| "Invalid legacy sample")?;
            if (used + left - 100.0).abs() > 0.000001 {
                return Err("Inconsistent legacy percentages".into());
            }
            let label = if window == "monthly" {
                "month".into()
            } else {
                crate::model::duration_label(mins)
            };
            let quota = crate::model::Quota::new(
                &window,
                label.clone(),
                if mins == 0 { None } else { Some(mins) },
                used,
                if reset == 0 { None } else { Some(reset) },
            )
            .map_err(|_| "Invalid legacy quota")?;
            tx.execute("INSERT INTO samples(provider,profile,sampled_at,window,label,duration_minutes,resets_at,used_percent,remaining_percent) VALUES(?1,'default',?2,?3,?4,?5,?6,?7,?8)",params![provider,at,window,label,quota.duration_minutes,quota.resets_at,used,left]).map_err(|_|"Cannot import row")?;
            latest.insert(window, (at, quota));
        }
        let mut snapshot = durable(&tx, config)?;
        if let Some(p) = snapshot.providers.iter_mut().find(|p| {
            p.provider == provider && p.profile == "default" && p.last_success_at.is_none()
        }) {
            let at = latest.values().map(|(at, _)| *at).max();
            p.quotas = latest
                .into_values()
                .filter(|(time, _)| Some(*time) == at)
                .map(|(_, q)| q)
                .collect();
            p.last_success_at = at;
            p.source = "legacy monitor history import".into();
            p.limitations
                .push("Imported historical sample; refresh required".into());
            p.age_state(now(), config.stale_seconds);
        }
        tx.execute("INSERT INTO imports(source) VALUES(?1)", [&identity])
            .map_err(|_| "Cannot record import")?;
        let json = serde_json::to_vec(&snapshot).map_err(|_| "Cannot serialize import")?;
        tx.execute("INSERT INTO state(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json",[std::str::from_utf8(&json).map_err(|_|"Invalid import JSON")?]).map_err(|_|"Cannot persist import")?;
        tx.commit().map_err(|_| "Cannot commit import")?;
        atomic_write(&loc.cache, &json)
    })();
    let _ = fs::remove_dir_all(temporary);
    result
}
