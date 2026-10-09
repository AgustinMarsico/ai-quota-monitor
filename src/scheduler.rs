use crate::{
    config::{Config, Locations},
    model::{now, ErrorKind, ProviderError, Snapshot},
    providers::{self, Reading},
    storage,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
pub fn backoff(failures: u32, interval: u64, maximum: u64, retry: Option<u64>, seed: u64) -> u64 {
    let base = interval
        .saturating_mul(1u64 << failures.min(16))
        .min(maximum);
    let jitter = seed % (base / 10 + 1);
    base.saturating_add(jitter)
        .min(maximum)
        .max(retry.unwrap_or(0).min(i64::MAX as u64))
}
fn apply_result(
    p: &mut crate::model::ProviderStatus,
    c: &crate::config::ProviderConfig,
    config: &Config,
    timestamp: i64,
    result: Result<Reading, ProviderError>,
) -> bool {
    p.last_attempt_at = Some(timestamp);
    let finished = now().max(timestamp);
    let success = match result {
        Ok(reading) => {
            p.quotas = reading.quotas;
            p.account = reading.account;
            p.model = reading.model;
            p.plan = reading.plan;
            p.source = reading.source;
            p.limitations = reading.limitations;
            p.last_success_at = Some(finished);
            p.error = None;
            p.consecutive_failures = 0;
            p.next_attempt_at = finished.saturating_add(c.interval_seconds as i64);
            true
        }
        Err(error) => {
            p.consecutive_failures = p.consecutive_failures.saturating_add(1);
            let interval = if matches!(
                error.kind,
                ErrorKind::Authentication | ErrorKind::CredentialsMissing
            ) {
                c.interval_seconds.max(600)
            } else {
                c.interval_seconds
            };
            p.next_attempt_at = finished.saturating_add(backoff(
                p.consecutive_failures,
                interval,
                config.max_backoff_seconds,
                error.retry_after_seconds,
                timestamp as u64,
            ) as i64);
            p.error = Some(error);
            false
        }
    };
    p.age_state(finished, config.stale_seconds);
    success
}
pub fn update_with<F>(
    config: &Config,
    loc: &Locations,
    provider: Option<&str>,
    manual: bool,
    timestamp: i64,
    mut fetch: F,
) -> Result<(Snapshot, bool), String>
where
    F: FnMut(&crate::config::ProviderConfig, i64) -> Result<Reading, ProviderError>,
{
    let _lock = storage::writer_lock(loc)?;
    let mut db = storage::open(loc)?;
    let mut snapshot = storage::durable(&db, config)?;
    let mut successes = vec![];
    let mut failed = false;
    let mut attempted = false;
    for c in config
        .providers
        .iter()
        .filter(|c| c.enabled && provider.is_none_or(|id| c.provider == id))
    {
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        let p = snapshot
            .providers
            .iter_mut()
            .find(|p| p.provider == c.provider && p.profile == c.profile)
            .ok_or("Missing profile")?;
        // Manual refresh can update fresh data, but cannot bypass an error backoff.
        if p.next_attempt_at > timestamp && (!manual || p.error.is_some()) {
            continue;
        }
        attempted = true;
        if apply_result(p, c, config, timestamp, fetch(c, timestamp)) {
            successes.push((p.provider.clone(), p.profile.clone()));
        } else {
            failed = true;
        }
    }
    if !attempted {
        if storage::read_cache(&loc.cache)
            .map_or(true, |cache| cache.generated_at != snapshot.generated_at)
        {
            storage::atomic_write(
                &loc.cache,
                &serde_json::to_vec(&snapshot).map_err(|_| "Cannot serialize recovered state")?,
            )?;
        }
        return Ok((snapshot, false));
    }
    snapshot.generated_at = now().max(timestamp);
    let alerts = storage::save(&mut db, loc, &snapshot, &successes, config)?;
    if config.notifications {
        for alert in alerts {
            notify(&alert);
        }
    }
    Ok((snapshot, failed))
}
pub fn refresh(
    config: &Config,
    loc: &Locations,
    provider: Option<&str>,
    manual: bool,
) -> Result<(Snapshot, bool), String> {
    update_with(config, loc, provider, manual, now(), providers::fetch)
}
fn notify(alert: &storage::Alert) {
    use std::os::unix::process::CommandExt;
    let child = std::process::Command::new("notify-send")
        .process_group(0)
        .args([
            "-a",
            "AI Quota Monitor",
            "-u",
            if alert.threshold <= 5 {
                "critical"
            } else {
                "normal"
            },
            "AI quota",
            &alert.message,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        eprintln!("notification unavailable; check notify-send");
        return;
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    eprintln!("notification delivery failed");
                }
                return;
            }
            _ if std::time::Instant::now() >= deadline => {
                // SAFETY: this notifier and its descendants own this new group.
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("notification delivery timed out");
                return;
            }
            _ => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}
pub fn check(
    config: &Config,
    loc: &Locations,
    provider: Option<&str>,
) -> Result<(Snapshot, bool), String> {
    let _lock = storage::existing_writer_lock(loc)?;
    let previous = if loc.cache.exists() {
        Some(storage::read_cache(&loc.cache)?)
    } else {
        None
    };
    let mut snapshot = storage::empty(config);
    let mut failed = false;
    for c in config
        .providers
        .iter()
        .filter(|c| c.enabled && provider.is_none_or(|id| c.provider == id))
    {
        let p = snapshot
            .providers
            .iter_mut()
            .find(|p| p.provider == c.provider && p.profile == c.profile)
            .ok_or("Missing profile")?;
        if let Some(old) = previous.as_ref().and_then(|s| {
            s.providers
                .iter()
                .find(|p| p.provider == c.provider && p.profile == c.profile)
        }) {
            *p = old.clone();
        }
        if p.error.is_some() && p.next_attempt_at > now() {
            failed = true;
            continue;
        }
        let timestamp = now();
        failed |= !apply_result(p, c, config, timestamp, providers::fetch(c, timestamp));
    }
    snapshot.generated_at = now();
    Ok((snapshot, failed))
}
pub fn daemon(config: &Config, loc: &Locations) -> Result<(), String> {
    storage::private_dir(&loc.state)?;
    let daemon_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(loc.state.join("daemon.lock"))
        .map_err(|_| "Cannot open daemon lock")?;
    daemon_lock
        .try_lock()
        .map_err(|_| "Daemon already running")?;
    // SAFETY: handler only stores an atomic flag; no allocation or I/O in signal context.
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
    }
    eprintln!(
        "ai-quota daemon started; {} enabled profiles",
        config.providers.iter().filter(|c| c.enabled).count()
    );
    let mut next_log = 0;
    while !STOP.load(Ordering::Relaxed) {
        let result = refresh(config, loc, None, false);
        if now() >= next_log {
            match result {
                Ok((snapshot, _)) => {
                    for p in snapshot.providers.iter().filter(|p| p.error.is_some()) {
                        eprintln!(
                            "provider={} profile={} update failed; next_attempt_at={}",
                            p.provider, p.profile, p.next_attempt_at
                        );
                    }
                }
                Err(e) => eprintln!("monitor: {e}"),
            }
            next_log = now() + 60;
        }
        for _ in 0..5 {
            if STOP.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    Ok(())
}
