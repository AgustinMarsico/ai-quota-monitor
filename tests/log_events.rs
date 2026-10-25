use ai_quota_monitor::{
    config::{Config, GoAuth, Locations},
    model::{now, ErrorKind, ProviderError, Quota},
    providers::{go, Reading},
    scheduler::{self, EventLogger, UpdateEvent, UpdateEventKind},
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
    config.providers.retain(|p| p.provider == "opencode-go");
    (dir, loc, config)
}

fn reading(source: &str) -> Reading {
    Reading {
        account: None,
        model: None,
        plan: None,
        source: source.into(),
        limitations: vec![],
        quotas: vec![Quota::new("rolling", "5h".into(), Some(300), 12.0, None).unwrap()],
    }
}

#[test]
fn log_only_actual_attempts_and_first_backoff_transition() {
    let (_dir, loc, config) = fixture();
    let timestamp = now();
    let mut logger = EventLogger::new(false);

    let (_, failed, events) =
        scheduler::update_with_events(&config, &loc, None, false, timestamp, |_, _| {
            Ok(reading("https://opencode.ai/zen/go/v1/usage"))
        })
        .unwrap();
    assert!(!failed);
    assert_eq!(
        logger.format(events),
        ["INFO provider=opencode-go profile=default event=query_succeeded"]
    );

    let (snapshot, failed, events) =
        scheduler::update_with_events(&config, &loc, None, true, timestamp + 1, |_, _| {
            Err(ProviderError::new(
                ErrorKind::Authentication,
                "password=never-log-this-secret",
            ))
        })
        .unwrap();
    assert!(failed);
    let deadline = snapshot.providers[0].next_attempt_at;
    assert!(deadline > timestamp + 2);
    let lines = logger.format(events);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("event=query_failed cause=authentication failures=1"));
    assert!(lines[1].contains("event=backoff_started"));
    assert!(!lines.join(" ").contains("never-log-this-secret"));

    for t in timestamp + 2..timestamp + 7 {
        let (state, failed, events) =
            scheduler::update_with_events(&config, &loc, None, false, t, |_, _| {
                panic!("backoff must not fetch")
            })
            .unwrap();
        assert!(!failed);
        assert_eq!(state.providers[0].next_attempt_at, deadline);
        assert!(logger.format(events).is_empty());
    }

    // First genuine success after the error is a recovery, not an ordinary success.
    let (_, failed, events) =
        scheduler::update_with_events(&config, &loc, None, false, deadline, |_, _| {
            Ok(reading("https://opencode.ai/zen/go/v1/usage"))
        })
        .unwrap();
    assert!(!failed);
    assert_eq!(
        logger.format(events),
        ["INFO provider=opencode-go profile=default event=recovered"]
    );
    let db = storage::open(&loc).unwrap();
    let samples: i64 = db
        .query_row("SELECT COUNT(*) FROM samples", [], |row| row.get(0))
        .unwrap();
    assert_eq!(samples, 2);
}

#[test]
fn debug_skip_is_emitted_once_per_retry_deadline() {
    let mut logger = EventLogger::new(true);
    let skipped = |retry| UpdateEvent {
        provider: "opencode-go".into(),
        profile: "default".into(),
        kind: UpdateEventKind::SkippedBackoff {
            next_attempt_at: retry,
        },
    };
    assert_eq!(logger.format([skipped(100)]).len(), 1);
    assert!(logger.format([skipped(100)]).is_empty());
    assert_eq!(logger.format([skipped(200)]).len(), 1);
    // A fresh attempt starts a new debug period even with the same retry time.
    logger.format([UpdateEvent {
        provider: "opencode-go".into(),
        profile: "default".into(),
        kind: UpdateEventKind::Succeeded { recovered: true },
    }]);
    assert_eq!(logger.format([skipped(200)]).len(), 1);
}

#[test]
fn auth_change_emits_transition_and_a_real_request_without_exposing_secrets() {
    let (_dir, loc, mut config) = fixture();
    let timestamp = now();
    scheduler::update_with(&config, &loc, None, false, timestamp, |_, _| {
        Err(ProviderError::new(
            ErrorKind::Authentication,
            "token=private",
        ))
    })
    .unwrap();
    config.providers[0].go_auth = GoAuth::ConsoleOauth;
    let (_, failed, events) =
        scheduler::update_with_events(&config, &loc, None, false, timestamp + 1, |_, _| {
            Ok(reading(go::CONSOLE_USAGE_URL))
        })
        .unwrap();
    assert!(!failed);
    let mut logger = EventLogger::new(false);
    let lines = logger.format(events);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        "INFO provider=opencode-go profile=default event=auth_changed from=api_key to=console_oauth"
    );
    assert!(lines[1].contains("event=recovered"));
    assert!(!lines.join(" ").contains("private"));
}

#[test]
fn unchanged_error_after_restart_is_only_debug_skip() {
    let (_dir, loc, config) = fixture();
    let at = now();
    scheduler::update_with(&config, &loc, None, false, at, |_, _| {
        Err(ProviderError::new(ErrorKind::Network, "outage"))
    })
    .unwrap();
    // A fresh daemon starts with a persisted backoff, not a new failure.
    let (_, _, events) =
        scheduler::update_with_events(&config, &loc, None, false, at + 1, |_, _| {
            panic!("must not retry")
        })
        .unwrap();
    let mut normal = EventLogger::new(false);
    assert!(normal.format(events.clone()).is_empty());
    let mut debug = EventLogger::new(true);
    assert_eq!(debug.format(events).len(), 1);
}
