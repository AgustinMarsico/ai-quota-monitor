use ai_quota_monitor::{
    config::{Config, Locations},
    model::{now, ErrorKind, ProviderError, Quota},
    providers::{go, Reading},
    scheduler, storage,
};
#[test]
fn retry_after_http_date() {
    let e = go::response(
        b"HTTP/2 429\r\nRetry-After: Thu, 01 Jan 1970 00:30:00 GMT\r\n\r\n\n__AI_QUOTA_HTTP__429",
        1000,
    )
    .err()
    .unwrap();
    assert_eq!(e.retry_after_seconds, Some(800));
    assert_eq!(scheduler::backoff(1, 180, 3600, Some(172800), 0), 172800);
}
#[test]
fn known_period_alerts_and_window_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations {
        state: dir.path().join("state"),
        cache: dir.path().join("cache/status.json"),
        config: dir.path().join("config.json"),
    };
    let c = Config {
        notifications: false,
        ..Config::default()
    };
    let mut db = storage::open(&loc).unwrap();
    let mut s = storage::empty(&c);
    s.providers[0].last_success_at = Some(now());
    let ids = vec![("codex".into(), "default".into())];
    for (left, reset, alerts) in [
        (4.0, 2000000000, 1),
        (50.0, 2000000001, 0),
        (18.0, 2000000002, 0),
        (18.0, 2000010000, 1),
    ] {
        s.providers[0].quotas =
            vec![Quota::new("primary", "5h".into(), Some(300), 100.0 - left, Some(reset)).unwrap()];
        assert_eq!(
            storage::save(&mut db, &loc, &s, &ids, &c).unwrap().len(),
            alerts
        );
    }
    s.providers[0].quotas.push(
        Quota::new(
            "secondary",
            "7d".into(),
            Some(10080),
            96.0,
            Some(2000010000),
        )
        .unwrap(),
    );
    assert_eq!(storage::save(&mut db, &loc, &s, &ids, &c).unwrap().len(), 1);
}
#[test]
fn simulated_resume_polls_once_not_catchup() {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations {
        state: dir.path().join("state"),
        cache: dir.path().join("cache/status.json"),
        config: dir.path().join("config.json"),
    };
    let mut c = Config {
        notifications: false,
        ..Config::default()
    };
    c.providers.truncate(1);
    let at = now();
    let mut calls = 0;
    let mut fetch = |_: &ai_quota_monitor::config::ProviderConfig, _| {
        calls += 1;
        Ok(Reading {
            quotas: vec![Quota::new("primary", "5h".into(), Some(300), 22.0, None).unwrap()],
            plan: None,
            source: "fixture".into(),
            limitations: vec![],
        })
    };
    scheduler::update_with(&c, &loc, None, false, at, &mut fetch).unwrap();
    scheduler::update_with(&c, &loc, None, false, at + 86400, &mut fetch).unwrap();
    scheduler::update_with(&c, &loc, None, false, at + 86401, &mut fetch).unwrap();
    assert_eq!(calls, 2);
}
#[test]
fn no_idle_disk_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let loc = Locations {
        state: dir.path().join("state"),
        cache: dir.path().join("cache/status.json"),
        config: dir.path().join("config.json"),
    };
    let mut c = Config {
        notifications: false,
        ..Config::default()
    };
    c.providers.truncate(1);
    let at = now();
    scheduler::update_with(&c, &loc, None, false, at, |_, _| {
        Err(ProviderError::new(ErrorKind::Network, "offline"))
    })
    .unwrap();
    let mtime = std::fs::metadata(&loc.cache).unwrap().modified().unwrap();
    scheduler::update_with(&c, &loc, None, false, at + 1, |_, _| panic!("not due")).unwrap();
    assert_eq!(
        std::fs::metadata(&loc.cache).unwrap().modified().unwrap(),
        mtime
    );
}
