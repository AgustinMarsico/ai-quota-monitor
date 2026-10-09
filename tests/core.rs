use ai_quota_monitor::{
    config::{Config, Locations, ProviderConfig},
    model::{DataState, ErrorKind, ProviderError, Quota},
    providers::{codex, go, Provider, Reading},
    scheduler, storage,
};
use serde_json::json;
fn env() -> (tempfile::TempDir, Locations, Config) {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations {
        state: dir.path().join("state"),
        cache: dir.path().join("cache/status.json"),
        config: dir.path().join("config.json"),
    };
    let config = Config {
        notifications: false,
        ..Config::default()
    };
    (dir, loc, config)
}
fn reading(left: f64, reset: Option<i64>) -> Reading {
    Reading {
        account: None,
        model: None,
        quotas: vec![Quota::new("primary", "5h".into(), Some(300), 100.0 - left, reset).unwrap()],
        plan: None,
        source: "fixture".into(),
        limitations: vec![],
    }
}
#[test]
fn codex_normalizes_windows() {
    let r=codex::parse(&json!({"rateLimits":{"primary":{"usedPercent":22,"windowDurationMins":300,"resetsAt":2000000000},"secondary":{"usedPercent":38,"windowDurationMins":10080}}})).unwrap();
    assert_eq!(r.quotas[0].remaining_percent, 78.0);
    assert_eq!(r.quotas[1].label, "7d");
    assert_eq!(r.quotas[1].resets_at, None);
}
#[test]
fn codex_prefers_bucket() {
    let r = codex::parse(
        &json!({"rateLimits":{},"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":0}}}}),
    )
    .unwrap();
    assert_eq!(r.quotas[0].duration_minutes, None);
}
#[test]
fn codex_rejects_wrong_bucket() {
    assert!(
        codex::parse(&json!({"rateLimits":{"limitId":"other","primary":{"usedPercent":0}}}))
            .is_err()
    );
}
#[test]
fn go_percentage_units_and_month() {
    let r=go::parse(&json!({"usage":{"rolling":{"percent":1,"resetInSec":0},"weekly":{"percent":55},"monthly":{"percent":99}}}),1000).unwrap();
    assert_eq!(r.quotas[0].remaining_percent, 99.0);
    assert_eq!(r.quotas[0].resets_at, Some(1000));
    assert_eq!(r.quotas[2].duration_minutes, None);
}
#[test]
fn incomplete_windows_are_not_zero() {
    let r = go::parse(
        &json!({"usage":{"weekly":{"usagePercent":0.5},"monthly":null}}),
        1000,
    )
    .unwrap();
    assert_eq!(r.quotas.len(), 1);
    assert_eq!(r.quotas[0].resets_at, None);
}
#[test]
fn invalid_values_rejected() {
    assert!(codex::parse(
        &json!({"rateLimits":{"primary":{"usedPercent":5,"windowDurationMins":"bad"}}})
    )
    .is_err());
    for value in [-1.0, 101.0, f64::NAN, f64::INFINITY] {
        assert!(Quota::new("x", "x".into(), None, value, None).is_err());
    }
    assert!(go::parse(
        &json!({"usage":{"rolling":{"percent":5,"resetInSec":-1}}}),
        1000
    )
    .is_err());
    assert!(codex::parse(&json!({"rateLimits":{"primary":{"usedPercent":"secret"}}})).is_err());
}
#[test]
fn empty_response_unavailable() {
    assert!(go::parse(&json!({"usage":{}}), 1000).is_err());
    assert!(codex::parse(&json!({"rateLimits":{}})).is_err());
}
#[test]
fn http_errors_and_retry_after() {
    let e = go::response(
        b"HTTP/2 429\r\nRetry-After: 1200\r\n\r\nSECRET\n__AI_QUOTA_HTTP__429",
        1000,
    )
    .err()
    .unwrap();
    assert_eq!(e.kind, ErrorKind::RateLimited);
    assert_eq!(e.retry_after_seconds, Some(1200));
    assert!(!e.message.contains("SECRET"));
    for status in ["401", "403"] {
        let e = go::response(
            format!("HTTP/2 {status}\r\n\r\nTOKEN\n__AI_QUOTA_HTTP__{status}").as_bytes(),
            1000,
        )
        .err()
        .unwrap();
        assert_eq!(e.kind, ErrorKind::Authentication);
    }
}
#[test]
fn http_valid_and_invalid_json() {
    let r=go::response(b"HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 200\r\n\r\n{\"usage\":{\"rolling\":{\"percent\":9}}}\n__AI_QUOTA_HTTP__200",1000).unwrap();
    assert_eq!(r.quotas[0].remaining_percent, 91.0);
    assert!(go::response(b"not json\n__AI_QUOTA_HTTP__200", 1000).is_err());
}
#[test]
fn credentials_missing_and_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = ProviderConfig {
        auth_file: Some(dir.path().join("auth.json")),
        ..ProviderConfig::default()
    };
    // Explicit override injection keeps tests independent from user credentials.
    assert_eq!(
        go::credentials_with_override(&p, None).unwrap_err().kind,
        ErrorKind::CredentialsMissing
    );
    std::fs::write(
        p.auth_file.as_ref().unwrap(),
        b"{\"opencode-go\":{\"key\":\"secret\\nheader\"}}",
    )
    .unwrap();
    assert_eq!(
        go::credentials_with_override(&p, None).unwrap_err().kind,
        ErrorKind::Authentication
    );
    p.auth_file = Some(dir.path().join("missing"));
    assert!(go::credentials_with_override(&p, None).is_err());
}
#[test]
fn stale_and_expired_window() {
    let mut p = ai_quota_monitor::model::ProviderStatus::empty("codex", "default");
    p.quotas = reading(40.0, Some(2000)).quotas;
    p.last_success_at = Some(1000);
    p.age_state(1100, 600);
    assert_eq!(p.state, DataState::Fresh);
    p.age_state(1700, 600);
    assert_eq!(p.state, DataState::Stale);
    p.last_success_at = Some(1999);
    p.age_state(2000, 600);
    assert_eq!(p.state, DataState::Stale);
    assert_eq!(p.quotas[0].remaining_percent, 40.0);
}
#[test]
fn failure_preserves_last_success_and_restart_backoff() {
    let (_dir, loc, config) = env();
    let at = ai_quota_monitor::model::now();
    scheduler::update_with(&config, &loc, Some("codex"), true, at, |_, _| {
        Ok(reading(78.0, None))
    })
    .unwrap();
    let (s, failed) = scheduler::update_with(&config, &loc, Some("codex"), true, at + 1, |_, _| {
        Err(ProviderError::new(ErrorKind::Network, "offline"))
    })
    .unwrap();
    assert!(failed);
    let p = &s.providers[0];
    assert_eq!(p.quotas[0].remaining_percent, 78.0);
    assert_eq!(p.state, DataState::Stale);
    let next = p.next_attempt_at;
    scheduler::update_with(&config, &loc, Some("codex"), true, at + 2, |_, _| {
        panic!("must respect persisted backoff")
    })
    .unwrap();
    let (s, _) = scheduler::update_with(&config, &loc, Some("codex"), false, next, |_, _| {
        Ok(reading(60.0, None))
    })
    .unwrap();
    assert_eq!(s.providers[0].error, None);
    assert_eq!(s.providers[0].consecutive_failures, 0);
}
#[test]
fn backoff_and_retry_bound() {
    assert_eq!(scheduler::backoff(30, 180, 3600, None, 0), 3600);
    assert_eq!(scheduler::backoff(1, 180, 3600, Some(7200), 0), 7200);
}
#[test]
fn competing_writers_excluded() {
    let (_d, loc, _c) = env();
    let lock = storage::writer_lock(&loc).unwrap();
    assert!(storage::writer_lock(&loc).is_err());
    drop(lock);
    assert!(storage::writer_lock(&loc).is_ok());
}
#[test]
fn concurrent_cache_reads_are_complete() {
    let (_d, loc, config) = env();
    let s = storage::empty(&config);
    storage::atomic_write(&loc.cache, &serde_json::to_vec(&s).unwrap()).unwrap();
    let path = loc.cache.clone();
    let reader = std::thread::spawn(move || {
        for _ in 0..500 {
            storage::read_cache(&path).unwrap().validate().unwrap();
        }
    });
    for _ in 0..100 {
        storage::atomic_write(&loc.cache, &serde_json::to_vec(&s).unwrap()).unwrap();
    }
    reader.join().unwrap();
}
#[test]
fn durable_recovers_deleted_cache() {
    let (_d, loc, c) = env();
    scheduler::update_with(
        &c,
        &loc,
        Some("codex"),
        true,
        ai_quota_monitor::model::now(),
        |_, _| Ok(reading(62.0, None)),
    )
    .unwrap();
    std::fs::remove_file(&loc.cache).unwrap();
    scheduler::update_with(
        &c,
        &loc,
        None,
        false,
        ai_quota_monitor::model::now(),
        |p, _| {
            if p.provider == "codex" {
                panic!("not due")
            } else {
                Err(ProviderError::new(ErrorKind::CredentialsMissing, "missing"))
            }
        },
    )
    .unwrap();
    assert_eq!(
        storage::read_cache(&loc.cache).unwrap().providers[0].quotas[0].remaining_percent,
        62.0
    );
}
#[test]
fn contract_validation() {
    let (_d, _l, c) = env();
    let mut s = storage::empty(&c);
    s.schema_version = 2;
    assert!(s.validate().is_err());
    s.schema_version = 1;
    s.providers[0].quotas = reading(30.0, None).quotas;
    s.providers[0].quotas[0].remaining_percent = 99.0;
    assert!(s.validate().is_err());
}
#[test]
fn configuration_validates_profiles() {
    let mut c = Config::default();
    c.providers.push(c.providers[0].clone());
    assert!(c.validate().is_err());
    assert!(serde_json::from_str::<Config>("{\"token\":\"x\"}").is_err());
}
#[test]
fn alerts_deduplicate_and_rearm_without_fake_reset() {
    let (_d, loc, c) = env();
    let mut db = storage::open(&loc).unwrap();
    let mut s = storage::empty(&c);
    s.providers[0].last_success_at = Some(ai_quota_monitor::model::now());
    let ids = vec![("codex".into(), "default".into())];
    for (remaining, count) in [
        (18.0, 1),
        (18.0, 0),
        (4.0, 1),
        (4.0, 0),
        (50.0, 0),
        (18.0, 1),
    ] {
        s.providers[0].quotas = reading(remaining, None).quotas;
        assert_eq!(
            storage::save(&mut db, &loc, &s, &ids, &c).unwrap().len(),
            count
        );
    }
    drop(db);
    let mut db = storage::open(&loc).unwrap();
    assert!(storage::save(&mut db, &loc, &s, &ids, &c)
        .unwrap()
        .is_empty());
}
#[test]
fn csv_and_cli_json() {
    let (_d, loc, c) = env();
    scheduler::update_with(
        &c,
        &loc,
        Some("codex"),
        true,
        ai_quota_monitor::model::now(),
        |_, _| Ok(reading(78.0, None)),
    )
    .unwrap();
    assert!(storage::history(&loc, 7, true, None)
        .unwrap()
        .contains("78"));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_ai-quota"))
        .args([
            "status",
            "--cache",
            loc.cache.to_str().unwrap(),
            "--config",
            loc.config.to_str().unwrap(),
            "--json",
            "--provider",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["providers"].as_array().unwrap().len(), 1);
    let text = ai_quota_monitor::cli::render(
        &storage::read_cache(&loc.cache).unwrap(),
        true,
        "remaining",
        false,
    );
    assert!(text.contains("78%"));
    assert!(!text.contains('\x1b'));
}
#[test]
fn private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, loc, c) = env();
    scheduler::update_with(
        &c,
        &loc,
        Some("codex"),
        true,
        ai_quota_monitor::model::now(),
        |_, _| Ok(reading(90.0, None)),
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(&loc.cache).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&loc.state).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
#[test]
fn real_codex_transport_mock() {
    let config = ProviderConfig {
        codex_bin: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/mock-codex.py"),
        ..ProviderConfig::default()
    };
    let reading = codex::Codex.fetch(&config, 1000).unwrap();
    assert_eq!(reading.quotas[0].remaining_percent, 78.0);
}
