use ai_quota_monitor::{model::ErrorKind, providers::go::parse_console};
use serde_json::json;

#[test]
fn console_http_errors_never_echo_server_secrets() {
    for (status, kind) in [
        ("401", ErrorKind::Authentication),
        ("403", ErrorKind::Authentication),
        ("429", ErrorKind::RateLimited),
        ("500", ErrorKind::Unavailable),
        ("302", ErrorKind::Unavailable),
    ] {
        let body = format!(
            "HTTP/2 {status}\r\nRetry-After: 30\r\n\r\nSECRET_COOKIE\n__AI_QUOTA_HTTP__{status}"
        );
        let error = ai_quota_monitor::providers::go::console_response(body.as_bytes(), 1000)
            .err()
            .unwrap();
        assert_eq!(error.kind, kind);
        assert!(!error.message.contains("SECRET_COOKIE"));
        if status == "429" {
            assert_eq!(error.retry_after_seconds, Some(30));
        }
    }
    assert!(ai_quota_monitor::providers::go::console_response(
        b"<html>login</html>\n__AI_QUOTA_HTTP__200",
        1000
    )
    .is_err());
}

#[test]
fn console_windows_and_real_resets() {
    let result = parse_console(&json!({"access": {"endsAt":"2026-11-01T00:00:00Z", "meters": {
        "fiveHour":{"usedMicroCents":"300", "limitMicroCents":"1200", "resetsAt":"2026-10-20T15:00:00.000Z"},
        "week":{"usedMicroCents":400, "limitMicroCents":1000},
        "month":{"usedMicroCents":"1", "limitMicroCents":"1000", "resetsAt":null}
    }}})).unwrap();
    assert_eq!(result.quotas.len(), 3);
    assert_eq!(result.quotas[0].remaining_percent, 75.0);
    assert_eq!(result.quotas[0].duration_minutes, Some(300));
    assert!(result.quotas[0].resets_at.is_some());
    assert_eq!(result.quotas[1].remaining_percent, 60.0);
    assert_eq!(result.quotas[1].duration_minutes, Some(10080));
    assert_eq!(result.quotas[2].remaining_percent, 99.9);
    assert_eq!(result.quotas[2].resets_at, None);
    assert_eq!(result.quotas[2].duration_minutes, None);
}

#[test]
fn console_missing_windows_are_not_zero_usage() {
    let result = parse_console(&json!({"access":{"meters":{"week":{
        "usedMicroCents":"1", "limitMicroCents":"100"
    }}}}))
    .unwrap();
    assert_eq!(result.quotas.len(), 1);
    assert_eq!(result.quotas[0].window, "weekly");
    for value in [
        json!(null),
        json!({"access":null}),
        json!({"balanceMicroCents":"100"}),
        json!({"access":{"meters":{}}}),
    ] {
        assert_eq!(
            parse_console(&value).err().unwrap().kind,
            ErrorKind::Unavailable
        );
    }
}

#[test]
fn console_rejects_malformed_meters_without_guessing_units() {
    for meter in [
        json!({"usedMicroCents":1}),
        json!({"usedMicroCents":1,"limitMicroCents":0}),
        json!({"usedMicroCents":-1,"limitMicroCents":100}),
        json!({"usedMicroCents":"NaN","limitMicroCents":100}),
        json!({"usedMicroCents":101,"limitMicroCents":100}),
        json!({"usedMicroCents":1,"limitMicroCents":100,"resetsAt":"tomorrow"}),
        json!({"percent":0.5}),
    ] {
        assert_eq!(
            parse_console(&json!({"access":{"meters":{"fiveHour":meter}}}))
                .err()
                .unwrap()
                .kind,
            ErrorKind::InvalidResponse
        );
    }
}
