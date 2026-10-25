use ai_quota_monitor::{
    config::{Config, Locations, NotificationPolicyConfig},
    model::Quota,
    storage,
};

fn fixture() -> (tempfile::TempDir, Locations, Config) {
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
    config.providers.truncate(1);
    (dir, loc, config)
}

#[test]
fn published_notification_example_is_valid_without_credentials() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/notifications.example.json");
    let config = Config::load(&path).unwrap();
    assert!(
        !config
            .notification_transports
            .ntfy
            .as_ref()
            .unwrap()
            .enabled
    );
    assert!(config.notification_policy.unwrap().events.monitor_failed);
}

#[test]
fn published_console_example_is_valid_without_credentials() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/go-console.example.json");
    let config = Config::load(&path).unwrap();
    assert_eq!(config.providers.len(), 2);
    assert_eq!(
        config.providers[1].go_auth,
        ai_quota_monitor::config::GoAuth::ConsoleOauth
    );
}

fn observe(
    db: &mut rusqlite::Connection,
    loc: &Locations,
    config: &Config,
    at: i64,
    used: f64,
    reset: Option<i64>,
) -> Vec<ai_quota_monitor::notifications::Notification> {
    let mut snapshot = storage::durable(db, config).unwrap();
    snapshot.generated_at = at;
    snapshot.providers[0].last_success_at = Some(at);
    snapshot.providers[0].quotas =
        vec![Quota::new("primary", "5h".into(), Some(300), used, reset).unwrap()];
    storage::save(
        db,
        loc,
        &snapshot,
        &[("codex".into(), "default".into())],
        config,
    )
    .unwrap()
}

#[test]
fn migration_twice_preserves_history_and_does_not_replay_old_alerts() {
    let (_dir, loc, mut config) = fixture();
    let mut db = storage::open(&loc).unwrap();
    assert_eq!(
        observe(&mut db, &loc, &config, 1000, 96.0, Some(2000)).len(),
        1
    );
    let before: i64 = db
        .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
        .unwrap();
    drop(db);
    config.notification_policy = Some(NotificationPolicyConfig::default());
    for _ in 0..2 {
        let mut db = storage::open(&loc).unwrap();
        assert!(observe(&mut db, &loc, &config, 1100, 96.0, Some(2000)).is_empty());
    }
    let mut db = storage::open(&loc).unwrap();
    let events = observe(&mut db, &loc, &config, 1200, 100.0, Some(2000));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].threshold, 100.0);
    let after: i64 = db
        .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))
        .unwrap();
    assert_eq!(after, before + 3);
    assert_eq!(
        observe(&mut db, &loc, &config, 1300, 99.0, Some(2000)).len(),
        0
    );
    assert_eq!(
        observe(&mut db, &loc, &config, 1400, 100.0, Some(2000)).len(),
        0
    );
}

#[test]
fn disabled_delivery_advances_policy_and_reactivation_has_no_backlog() {
    let (_dir, loc, mut config) = fixture();
    config.notification_policy = Some(NotificationPolicyConfig::default());
    let mut db = storage::open(&loc).unwrap();
    assert_eq!(observe(&mut db, &loc, &config, 1000, 95.0, None).len(), 1);
    config.notifications = true;
    assert!(observe(&mut db, &loc, &config, 1100, 95.0, None).is_empty());
}

#[test]
fn monitor_attempts_are_durable_and_skips_do_not_count() {
    let (_dir, loc, mut config) = fixture();
    let mut policy = NotificationPolicyConfig::default();
    policy.events.monitor_failed = true;
    policy.events.monitor_recovered = true;
    config.notification_policy = Some(policy);
    let mut db = storage::open(&loc).unwrap();
    let snapshot = storage::empty(&config);
    let failures = |at| {
        vec![storage::Attempt {
            provider: "codex".into(),
            profile: "default".into(),
            at,
            succeeded: false,
            auth_changed: false,
        }]
    };
    assert!(
        storage::save_with_attempts(&mut db, &loc, &snapshot, &[], &failures(1000), &config)
            .unwrap()
            .is_empty()
    );
    assert!(
        storage::save_with_attempts(&mut db, &loc, &snapshot, &[], &failures(1300), &config)
            .unwrap()
            .is_empty()
    );
    assert!(
        storage::save_with_attempts(&mut db, &loc, &snapshot, &[], &[], &config)
            .unwrap()
            .is_empty()
    );
    drop(db);
    let mut db = storage::open(&loc).unwrap();
    assert_eq!(
        storage::save_with_attempts(&mut db, &loc, &snapshot, &[], &failures(1600), &config)
            .unwrap()
            .len(),
        1
    );
    assert!(
        storage::save_with_attempts(&mut db, &loc, &snapshot, &[], &failures(1900), &config)
            .unwrap()
            .is_empty()
    );
    let success = vec![storage::Attempt {
        provider: "codex".into(),
        profile: "default".into(),
        at: 2000,
        succeeded: true,
        auth_changed: false,
    }];
    assert_eq!(
        storage::save_with_attempts(&mut db, &loc, &snapshot, &[], &success, &config)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn failed_attempt_and_missing_window_interrupt_recovery() {
    let (_dir, loc, mut config) = fixture();
    config.notification_policy = Some(NotificationPolicyConfig::default());
    let mut db = storage::open(&loc).unwrap();
    assert_eq!(observe(&mut db, &loc, &config, 1000, 96.0, None).len(), 1);
    assert!(observe(&mut db, &loc, &config, 1100, 74.0, None).is_empty());
    let mut snapshot = storage::durable(&db, &config).unwrap();
    let failed = [storage::Attempt {
        provider: "codex".into(),
        profile: "default".into(),
        at: 1150,
        succeeded: false,
        auth_changed: false,
    }];
    storage::save_with_attempts(&mut db, &loc, &snapshot, &[], &failed, &config).unwrap();
    assert!(observe(&mut db, &loc, &config, 1200, 73.0, None).is_empty());
    snapshot = storage::durable(&db, &config).unwrap();
    snapshot.providers[0].quotas.clear();
    snapshot.providers[0].last_success_at = Some(1250);
    let success = [storage::Attempt {
        provider: "codex".into(),
        profile: "default".into(),
        at: 1250,
        succeeded: true,
        auth_changed: false,
    }];
    storage::save_with_attempts(
        &mut db,
        &loc,
        &snapshot,
        &[("codex".into(), "default".into())],
        &success,
        &config,
    )
    .unwrap();
    assert!(observe(&mut db, &loc, &config, 1300, 73.0, None).is_empty());
    let recovered = observe(&mut db, &loc, &config, 1400, 72.0, None);
    assert_eq!(recovered.len(), 1);
    assert!(recovered[0].title.contains("recovered"));
}

#[test]
fn duration_change_does_not_import_legacy_watermark() {
    let (_dir, loc, mut config) = fixture();
    let mut db = storage::open(&loc).unwrap();
    observe(&mut db, &loc, &config, 1000, 96.0, None);
    config.notification_policy = Some(NotificationPolicyConfig::default());
    let mut snapshot = storage::durable(&db, &config).unwrap();
    snapshot.providers[0].last_success_at = Some(1100);
    snapshot.providers[0].quotas =
        vec![Quota::new("primary", "10h".into(), Some(600), 96.0, None).unwrap()];
    let events = storage::save(
        &mut db,
        &loc,
        &snapshot,
        &[("codex".into(), "default".into())],
        &config,
    )
    .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].threshold, 95.0);
}

#[test]
fn old_exhausted_reading_is_not_replayed_on_first_configured_start() {
    let (_dir, loc, mut config) = fixture();
    let mut db = storage::open(&loc).unwrap();
    assert_eq!(
        observe(&mut db, &loc, &config, 1000, 100.0, Some(2000)).len(),
        1
    );
    config.notification_policy = Some(NotificationPolicyConfig::default());
    assert!(observe(&mut db, &loc, &config, 1100, 100.0, Some(2000)).is_empty());
    assert!(observe(&mut db, &loc, &config, 1200, 99.0, Some(2000)).is_empty());
    assert!(observe(&mut db, &loc, &config, 1300, 100.0, Some(2000)).is_empty());
}
