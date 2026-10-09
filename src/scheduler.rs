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
        let p = snapshot
            .providers
            .iter_mut()
            .find(|p| p.provider == c.provider && p.profile == c.profile)
            .ok_or("Missing profile")?;
        // Manual refresh can update fresh data, but cannot bypass an error backoff.
        if p.next_attempt_at > timestamp && (!manual || p.error.is_some()) {
            continue;
        }
        p.last_attempt_at = Some(timestamp);
        attempted = true;
        match fetch(c, timestamp) {
            Ok(reading) => {
                p.quotas = reading.quotas;
                p.plan = reading.plan;
                p.source = reading.source;
                p.limitations = reading.limitations;
                p.last_success_at = Some(now().max(timestamp));
                p.error = None;
                p.consecutive_failures = 0;
                p.next_attempt_at = now().max(timestamp) + c.interval_seconds as i64;
                successes.push((p.provider.clone(), p.profile.clone()));
            }
            Err(error) => {
                failed = true;
                p.consecutive_failures = p.consecutive_failures.saturating_add(1);
                let interval = if matches!(
                    error.kind,
                    ErrorKind::Authentication | ErrorKind::CredentialsMissing
                ) {
                    c.interval_seconds.max(600)
                } else {
                    c.interval_seconds
                };
                p.next_attempt_at = now().max(timestamp).saturating_add(backoff(
                    p.consecutive_failures,
                    interval,
                    config.max_backoff_seconds,
                    error.retry_after_seconds,
                    timestamp as u64,
                ) as i64);
                p.error = Some(error);
            }
        }
        p.age_state(now().max(timestamp), config.stale_seconds);
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
            let _ = std::process::Command::new("notify-send")
                .args(["-a", "AI Quota Monitor", "AI quota", &alert])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
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
