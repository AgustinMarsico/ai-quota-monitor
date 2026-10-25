// Endpoint and percentage semantics adapted from OpenCode Go Quota Monitor (MIT).
use super::{Provider, Reading};
use crate::{
    config::{xdg, ProviderConfig},
    model::{ErrorKind, ProviderError, Quota},
};
use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
};
pub struct Go;
fn invalid() -> ProviderError {
    ProviderError::new(
        ErrorKind::InvalidResponse,
        "Invalid OpenCode Go quota response",
    )
}
pub const CONSOLE_USAGE_URL: &str = "https://opencode.ai/console/api/go/status";

/// Console subscription meters only. Never substitute a prepaid balance or local estimates.
pub fn parse_console(value: &Value) -> Result<Reading, ProviderError> {
    let access = value
        .get("access")
        .filter(|v| !v.is_null())
        .ok_or_else(|| {
            ProviderError::new(
                ErrorKind::Unavailable,
                "Go subscription access not reported",
            )
        })?;
    let meters = access
        .get("meters")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    let mut quotas = Vec::new();
    for (field, window, label, minutes) in [
        ("fiveHour", "rolling", "5h", Some(300)),
        ("week", "weekly", "7d", Some(10080)),
        ("month", "monthly", "month", None),
    ] {
        let Some(meter) = meters.get(field).filter(|v| !v.is_null()) else {
            continue;
        };
        let amount = |key: &str| -> Result<f64, ProviderError> {
            let v = meter.get(key).ok_or_else(invalid)?;
            let n = if let Some(s) = v.as_str() {
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid());
                }
                s.parse::<f64>().map_err(|_| invalid())?
            } else {
                v.as_f64().ok_or_else(invalid)?
            };
            if !n.is_finite() || n < 0.0 {
                return Err(invalid());
            }
            Ok(n)
        };
        let limit = amount("limitMicroCents")?;
        let used = amount("usedMicroCents")?;
        if limit <= 0.0 {
            return Err(invalid());
        }
        let reset = meter.get("resetsAt").filter(|v| !v.is_null());
        // Do not treat the subscription's endsAt as a reported meter reset.
        let resets_at = reset
            .map(|v| {
                chrono::DateTime::parse_from_rfc3339(v.as_str().ok_or_else(invalid)?)
                    .map(|t| t.timestamp())
                    .map_err(|_| invalid())
            })
            .transpose()?;
        quotas.push(Quota::new(
            window,
            label.into(),
            minutes,
            used / limit * 100.0,
            resets_at,
        )?);
    }
    if quotas.is_empty() {
        return Err(ProviderError::new(
            ErrorKind::Unavailable,
            "Go returned no subscription meters",
        ));
    }
    Ok(Reading {
        quotas, account: None, model: None, plan: None,
        source: CONSOLE_USAGE_URL.into(),
        limitations: vec!["Console Go status is not a documented quota API; only reported subscription meters are shown; missing windows and resets are not estimated".into()],
    })
}
pub fn parse(value: &Value, timestamp: i64) -> Result<Reading, ProviderError> {
    let usage = value
        .get("usage")
        .filter(|v| v.is_object())
        .ok_or_else(invalid)?;
    let mut quotas = vec![];
    for (key, label, mins) in [
        ("rolling", "5h", Some(300)),
        ("weekly", "7d", Some(10080)),
        ("monthly", "month", None),
    ] {
        if let Some(v) = usage.get(key).filter(|v| !v.is_null()) {
            let used = v
                .get("percent")
                .or_else(|| v.get("usagePercent"))
                .and_then(Value::as_f64)
                .ok_or_else(invalid)?;
            let seconds = v
                .get("resetInSec")
                .or_else(|| v.get("resetInSeconds"))
                .filter(|v| !v.is_null())
                .map(|v| {
                    v.as_i64()
                        .filter(|n| (0..=366 * 86400).contains(n))
                        .ok_or_else(invalid)
                })
                .transpose()?;
            quotas.push(Quota::new(
                key,
                label.into(),
                mins,
                used,
                seconds.map(|n| timestamp.saturating_add(n)),
            )?);
        }
    }
    if quotas.is_empty() {
        return Err(ProviderError::new(
            ErrorKind::Unavailable,
            "OpenCode Go returned no quota windows",
        ));
    }
    Ok(Reading { quotas, account: None, model: None, plan: None, source: "https://opencode.ai/zen/go/v1/usage".into(), limitations: vec!["Undocumented endpoint; aggregate windows only, no per-model quotas inferred; reset may be absent".into()] })
}
pub fn credentials(config: &ProviderConfig) -> Result<String, ProviderError> {
    credentials_with_override(config, std::env::var("OPENCODE_API_KEY").ok().as_deref())
}
pub fn credentials_with_override(
    config: &ProviderConfig,
    override_key: Option<&str>,
) -> Result<String, ProviderError> {
    let key = if let Some(key) = override_key {
        key.to_owned()
    } else {
        let path = config
            .auth_file
            .clone()
            .unwrap_or_else(|| xdg("XDG_DATA_HOME", ".local/share").join("opencode/auth.json"));
        let data = std::fs::read(path).map_err(|_| {
            ProviderError::new(
                ErrorKind::CredentialsMissing,
                "Cannot read legacy OpenCode API key; Console V2 requires separate authentication; see docs/opencode-go.md",
            )
        })?;
        let auth: Value = serde_json::from_slice(&data).map_err(|_| {
            ProviderError::new(
                ErrorKind::Authentication,
                "Invalid OpenCode credential file (contents suppressed)",
            )
        })?;
        auth.get("opencode-go")
            .and_then(|v| v.get("key").or_else(|| v.get("apiKey")))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::new(
                    ErrorKind::CredentialsMissing,
                    "Legacy OpenCode Go API key missing; Console V2 requires separate authentication; see docs/opencode-go.md",
                )
            })?
            .to_owned()
    };
    if key.is_empty()
        || key.len() > 4096
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
    {
        return Err(ProviderError::new(
            ErrorKind::Authentication,
            "Invalid API key format (value suppressed)",
        ));
    }
    Ok(key)
}
pub fn response(bytes: &[u8], timestamp: i64) -> Result<Reading, ProviderError> {
    response_with_parser(bytes, timestamp, parse,
        "OpenCode Go rejected credentials; V2 Console sign-in does not update the legacy API key. See docs/opencode-go.md")
}
pub fn console_response(bytes: &[u8], timestamp: i64) -> Result<Reading, ProviderError> {
    response_with_parser(bytes, timestamp, |value, _| parse_console(value),
        "Console rejected the monitor's OAuth session; it may be expired or revoked. See docs/opencode-go.md to reauthorize")
}
fn response_with_parser(
    bytes: &[u8],
    timestamp: i64,
    parser: fn(&Value, i64) -> Result<Reading, ProviderError>,
    authentication_message: &str,
) -> Result<Reading, ProviderError> {
    if bytes.len() > 1_100_000 {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let (payload, status) = text
        .rsplit_once("\n__AI_QUOTA_HTTP__")
        .ok_or_else(invalid)?;
    let mut rest = payload;
    let mut retry = None;
    // curl --include can produce a proxy CONNECT header followed by the actual response.
    while rest.starts_with("HTTP/") {
        let (headers, body) = rest
            .split_once("\r\n\r\n")
            .or_else(|| rest.split_once("\n\n"))
            .ok_or_else(invalid)?;
        for line in headers.lines() {
            if let Some((key, value)) = line.split_once(':') {
                if key.eq_ignore_ascii_case("retry-after") {
                    retry = value.trim().parse::<u64>().ok().or_else(|| {
                        httpdate::parse_http_date(value.trim())
                            .ok()
                            .and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|duration| {
                                duration.as_secs().saturating_sub(timestamp.max(0) as u64)
                            })
                    });
                }
            }
        }
        rest = body;
    }
    match status.trim() {
        "200" => parser(
            &serde_json::from_str::<Value>(rest).map_err(|_| invalid())?,
            timestamp,
        ),
        "401" => Err(ProviderError::new(
            ErrorKind::Authentication,
            authentication_message,
        )),
        "403" => Err(ProviderError::new(
            ErrorKind::Authentication,
            "OpenCode Go denied workspace access or credential permission; reconnecting may not resolve a scope mismatch",
        )),
        "429" => Err(ProviderError {
            kind: ErrorKind::RateLimited,
            message: "OpenCode Go rate limited request".into(),
            retry_after_seconds: retry,
        }),
        _ => Err(ProviderError::new(
            ErrorKind::Unavailable,
            "OpenCode Go HTTP request failed; previous quota retained",
        )),
    }
}
impl Provider for Go {
    fn fetch(&self, config: &ProviderConfig, _: i64) -> Result<Reading, ProviderError> {
        if config.go_auth == crate::config::GoAuth::ConsoleOauth {
            return super::go_oauth::fetch(config, false);
        }
        let key = credentials(config)?;
        let mut child = Command::new("curl")
            .args([
                "-q",
                "--silent",
                "--no-location",
                "--proto",
                "=https",
                "--connect-timeout",
                "5",
                "--max-time",
                "20",
                "--max-filesize",
                "1048576",
                "--include",
                "--header",
                "Accept: application/json",
                "--user-agent",
                "ai-quota-monitor/0.1",
                "--write-out",
                "\n__AI_QUOTA_HTTP__%{http_code}",
                "--config",
                "-",
                "https://opencode.ai/zen/go/v1/usage",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ProviderError::new(ErrorKind::Unavailable, "Cannot start curl"))?;
        if child.stdin.take().is_none_or(|mut input| {
            writeln!(input, "header = \"Authorization: Bearer {key}\"").is_err()
        }) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ProviderError::new(
                ErrorKind::Internal,
                "Secure authentication pipe failed",
            ));
        }
        let output = child
            .wait_with_output()
            .map_err(|_| ProviderError::new(ErrorKind::Network, "HTTPS transport failed"))?;
        if !output.status.success() {
            return Err(ProviderError::new(
                ErrorKind::Network,
                "OpenCode Go connection failed or timed out",
            ));
        }
        // Relative resets are measured from response receipt, not request start.
        response(&output.stdout, crate::model::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_override_validates_without_reading_credential_files() {
        let p = ProviderConfig {
            auth_file: Some("/nonexistent-fixture-auth".into()),
            ..Default::default()
        };
        assert!(credentials_with_override(&p, Some("fixture-key")).is_ok());
        for bad in ["", "key\nheader", " key", "key\"injection"] {
            assert_eq!(
                credentials_with_override(&p, Some(bad)).err().unwrap().kind,
                ErrorKind::Authentication
            );
        }
    }
}
