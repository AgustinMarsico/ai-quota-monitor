use ai_quota_monitor::{
    config::{Config, GoAuth, Locations},
    model::{now, ErrorKind, ProviderError, Quota},
    providers::{go, Reading},
    scheduler, storage,
};

fn env() -> (tempfile::TempDir, Locations, Config) {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations {
        state: dir.path().join("state"),
        cache: dir.path().join("cache/status.json"),
        config: dir.path().join("config.json"),
    };
    let mut config = Config {
        notifications: false,
        ..Config::default()
    };
    config.providers.retain(|c| c.provider == "opencode-go");
    (dir, loc, config)
}

fn reading(source: &str, remaining: f64) -> Reading {
    Reading {
        account: None,
        model: None,
        plan: None,
        source: source.to_string(),
        limitations: vec![],
        quotas: vec![
            Quota::new("rolling", "5h".into(), Some(300), 100.0 - remaining, None).unwrap(),
        ],
    }
}

#[test]
fn change_of_auth_bypasses_backoff_once_and_keeps_history() {
    let (_dir, loc, mut config) = env();
    let at = now();
    let legacy_url = "https://opencode.ai/zen/go/v1/usage";
    scheduler::update_with(&config, &loc, None, false, at, |_, _| {
        Ok(reading(legacy_url, 18.0))
    })
    .unwrap();
    let (failed_state, failed) =
        scheduler::update_with(&config, &loc, None, true, at + 1, |_, _| {
            Err(ProviderError::new(ErrorKind::Authentication, "expired"))
        })
        .unwrap();
    assert!(failed);
    assert_eq!(failed_state.providers[0].go_auth, Some(GoAuth::ApiKey));
    let previous_success = failed_state.providers[0].last_success_at;
    let previous_retry = failed_state.providers[0].next_attempt_at;
    assert!(previous_retry > at + 2);

    config.providers[0].go_auth = GoAuth::ConsoleOauth;
    let mut calls = 0;
    let (recovered, failed) = scheduler::update_with(&config, &loc, None, false, at + 2, |_, _| {
        calls += 1;
        Ok(reading(go::CONSOLE_USAGE_URL, 18.0))
    })
    .unwrap();
    assert!(!failed);
    assert_eq!(calls, 1);
    let p = &recovered.providers[0];
    assert_eq!(p.go_auth, Some(GoAuth::ConsoleOauth));
    assert_eq!(p.source, go::CONSOLE_USAGE_URL);
    assert_eq!(p.consecutive_failures, 0);
    assert!(p.error.is_none());
    assert!(p.last_success_at.unwrap() >= previous_success.unwrap());
    assert_eq!(p.quotas[0].remaining_percent, 18.0);

    // The same auth mode still respects the new polling schedule.
    scheduler::update_with(&config, &loc, None, false, at + 3, |_, _| {
        panic!("unchanged authentication must not bypass the delay")
    })
    .unwrap();
    let db = storage::open(&loc).unwrap();
    let samples: i64 = db
        .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
        .unwrap();
    let alerts: i64 = db
        .query_row("SELECT COUNT(*) FROM alerts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(samples, 2);
    assert_eq!(alerts, 1); // Switching auth must not reset alert deduplication.

    // The persisted method survives process/DB reload and the reverse switch
    // gets one immediate attempt even when the previous method succeeded.
    config.providers[0].go_auth = GoAuth::ApiKey;
    let (rejected, failed) = scheduler::update_with(&config, &loc, None, false, at + 4, |_, _| {
        Err(ProviderError::new(ErrorKind::Authentication, "expired"))
    })
    .unwrap();
    assert!(failed);
    assert_eq!(rejected.providers[0].go_auth, Some(GoAuth::ApiKey));
    assert_eq!(rejected.providers[0].consecutive_failures, 1);
    scheduler::update_with(&config, &loc, None, false, at + 5, |_, _| {
        panic!("same failed authentication must respect the new backoff")
    })
    .unwrap();
}

#[test]
fn legacy_snapshot_infers_previous_method_without_resetting_quotas() {
    let (_dir, loc, mut config) = env();
    let at = now();
    let mut snapshot = storage::empty(&config);
    let p = &mut snapshot.providers[0];
    p.source = "https://opencode.ai/zen/go/v1/usage".into();
    p.quotas = reading(&p.source, 23.0).quotas;
    p.last_attempt_at = Some(at - 1);
    p.last_success_at = Some(at - 10);
    p.error = Some(ProviderError::new(
        ErrorKind::Authentication,
        "legacy expired",
    ));
    p.consecutive_failures = 9;
    p.next_attempt_at = at + 3600;
    assert_eq!(p.go_auth, None);
    let mut db = storage::open(&loc).unwrap();
    storage::save(&mut db, &loc, &snapshot, &[], &config).unwrap();
    drop(db);
    assert_eq!(
        storage::read_cache(&loc.cache).unwrap().providers[0].go_auth,
        None
    );

    // No configuration change => never bypass a legacy backoff.
    scheduler::update_with(&config, &loc, None, false, at, |_, _| {
        panic!("legacy api_key state must still respect its backoff")
    })
    .unwrap();

    // API key -> Console OAuth is detected even before go_auth was persisted.
    config.providers[0].go_auth = GoAuth::ConsoleOauth;
    let (result, failed) = scheduler::update_with(&config, &loc, None, false, at, |_, _| {
        Err(ProviderError::new(ErrorKind::Network, "transient"))
    })
    .unwrap();
    assert!(failed);
    let p = &result.providers[0];
    assert_eq!(p.go_auth, Some(GoAuth::ConsoleOauth));
    assert_eq!(p.consecutive_failures, 1); // Not 10 from the old key.
    assert_eq!(p.quotas[0].remaining_percent, 23.0);
    assert_eq!(p.last_success_at, Some(at - 10));
    assert!(p.next_attempt_at > at);
    scheduler::update_with(&config, &loc, None, false, at + 1, |_, _| {
        panic!("new failed method must not repeatedly bypass backoff")
    })
    .unwrap();
}

#[test]
fn unknown_legacy_auth_does_not_guess_or_bypass() {
    let (_dir, loc, mut config) = env();
    let at = now();
    let mut snapshot = storage::empty(&config);
    let p = &mut snapshot.providers[0];
    p.source = String::new();
    p.error = Some(ProviderError::new(
        ErrorKind::CredentialsMissing,
        "no credential",
    ));
    p.next_attempt_at = at + 1000;
    let mut db = storage::open(&loc).unwrap();
    storage::save(&mut db, &loc, &snapshot, &[], &config).unwrap();
    drop(db);
    config.providers[0].go_auth = GoAuth::ConsoleOauth;
    scheduler::update_with(&config, &loc, None, false, at, |_, _| {
        panic!("cannot infer auth method without a recorded method or known source")
    })
    .unwrap();
}

#[test]
fn go_auth_change_does_not_bypass_codex_backoff() {
    let (_dir, loc, mut config) = env();
    config
        .providers
        .push(ai_quota_monitor::config::ProviderConfig::default());
    let at = now();
    scheduler::update_with(&config, &loc, None, false, at, |_, _| {
        Err(ProviderError::new(ErrorKind::Network, "offline"))
    })
    .unwrap();
    let old = storage::read_cache(&loc.cache).unwrap();
    let codex_before = old
        .providers
        .iter()
        .find(|p| p.provider == "codex")
        .unwrap()
        .next_attempt_at;

    config.providers[0].go_auth = GoAuth::ConsoleOauth;
    let mut calls = 0;
    let (snapshot, _) = scheduler::update_with(&config, &loc, None, false, at + 1, |c, _| {
        assert_eq!(c.provider, "opencode-go");
        calls += 1;
        Ok(reading(go::CONSOLE_USAGE_URL, 73.0))
    })
    .unwrap();
    assert_eq!(calls, 1);
    let codex = snapshot
        .providers
        .iter()
        .find(|p| p.provider == "codex")
        .unwrap();
    assert_eq!(codex.next_attempt_at, codex_before);
    assert_eq!(codex.consecutive_failures, 1);
}
