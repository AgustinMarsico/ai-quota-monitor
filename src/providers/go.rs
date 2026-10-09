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
    Ok(Reading { quotas, plan: None, source: "https://opencode.ai/zen/go/v1/usage".into(), limitations: vec!["Undocumented endpoint; aggregate windows only, no per-model quotas inferred; reset may be absent".into()] })
}
pub fn credentials(config: &ProviderConfig) -> Result<String, ProviderError> {
    let key = if let Ok(key) = std::env::var("OPENCODE_API_KEY") {
        key
    } else {
        let path = config
            .auth_file
            .clone()
            .unwrap_or_else(|| xdg("XDG_DATA_HOME", ".local/share").join("opencode/auth.json"));
        let data = std::fs::read(path).map_err(|_| {
            ProviderError::new(
                ErrorKind::CredentialsMissing,
                "Cannot read OpenCode credentials; use /connect",
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
                    "OpenCode Go key missing; use /connect",
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
                    retry = value.trim().parse::<u64>().ok().map(|s| s.min(86400));
                }
            }
        }
        rest = body;
    }
    match status.trim() {
        "200" => parse(
            &serde_json::from_str::<Value>(rest).map_err(|_| invalid())?,
            timestamp,
        ),
        "401" | "403" => Err(ProviderError::new(
            ErrorKind::Authentication,
            "OpenCode Go rejected credentials; use /connect",
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
    fn fetch(&self, config: &ProviderConfig, timestamp: i64) -> Result<Reading, ProviderError> {
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
        response(&output.stdout, timestamp)
    }
}
