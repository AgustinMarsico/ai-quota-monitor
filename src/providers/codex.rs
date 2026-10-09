// Transport and parsing adapted from Codex Quota Monitor (MIT); see NOTICE.
use super::{Provider, Reading};
use crate::{
    config::ProviderConfig,
    model::{duration_label, ErrorKind, ProviderError, Quota},
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
pub struct Codex;
fn invalid() -> ProviderError {
    ProviderError::new(ErrorKind::InvalidResponse, "Invalid Codex quota response")
}
pub fn parse(value: &Value) -> Result<Reading, ProviderError> {
    let limits = value
        .get("rateLimitsByLimitId")
        .and_then(|v| v.get("codex"))
        .filter(|v| v.is_object())
        .or_else(|| value.get("rateLimits").filter(|v| v.is_object()))
        .ok_or_else(invalid)?;
    if limits
        .get("limitId")
        .and_then(Value::as_str)
        .is_some_and(|s| s != "codex")
    {
        return Err(invalid());
    }
    let mut quotas = vec![];
    for key in ["primary", "secondary"] {
        if let Some(v) = limits.get(key).filter(|v| !v.is_null()) {
            let used = v
                .get("usedPercent")
                .and_then(Value::as_f64)
                .ok_or_else(invalid)?;
            let mins = v.get("windowDurationMins").and_then(Value::as_u64);
            let reset = v
                .get("resetsAt")
                .filter(|v| !v.is_null())
                .map(|v| v.as_i64().ok_or_else(invalid))
                .transpose()?;
            quotas.push(Quota::new(
                key,
                mins.map(duration_label).unwrap_or_else(|| key.into()),
                mins,
                used,
                reset,
            )?);
        }
    }
    if quotas.is_empty() {
        return Err(ProviderError::new(
            ErrorKind::Unavailable,
            "Codex returned no quota windows; ChatGPT authentication is required",
        ));
    }
    Ok(Reading { quotas, plan: limits.get("planType").and_then(Value::as_str).map(str::to_owned), source: "codex app-server account/rateLimits/read".into(), limitations: vec!["Codex bucket only; bonus credits and conventional ChatGPT chat quotas are not inferred".into()] })
}
impl Provider for Codex {
    fn fetch(&self, config: &ProviderConfig, _: i64) -> Result<Reading, ProviderError> {
        let mut cmd = Command::new(&config.codex_bin);
        cmd.arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(home) = &config.codex_home {
            cmd.env("CODEX_HOME", home);
        }
        let mut child = cmd.spawn().map_err(|_| {
            ProviderError::new(
                ErrorKind::Unavailable,
                "Cannot start Codex CLI; check codex_bin",
            )
        })?;
        let result = (|| {
            let stdout = child.stdout.take().ok_or_else(invalid)?;
            let mut stdin = child.stdin.take().ok_or_else(invalid)?;
            let (tx, rx) = mpsc::sync_channel(8);
            let reader = std::thread::spawn(move || {
                let mut stream = BufReader::new(stdout);
                loop {
                    let mut line = vec![];
                    let result = stream.by_ref().take(1_048_577).read_until(b'\n', &mut line);
                    if !matches!(result, Ok(n) if n > 0) {
                        break;
                    }
                    if line.len() > 1_048_576 {
                        let _ = tx.send(Err(invalid()));
                        break;
                    }
                    if tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
            });
            let deadline = Instant::now() + Duration::from_secs(25);
            let receive = |id| -> Result<Value, ProviderError> {
                loop {
                    let line = rx
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .map_err(|_| {
                            ProviderError::new(
                                ErrorKind::Network,
                                "Codex timeout or disconnected transport",
                            )
                        })??;
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let msg: Value = serde_json::from_slice(&line).map_err(|_| invalid())?;
                    if msg.get("id").and_then(Value::as_i64) != Some(id) {
                        continue;
                    }
                    if let Some(error) = msg.get("error").filter(|v| !v.is_null()) {
                        // Never persist or print remote messages: they can include credentials.
                        let code = error.get("code").and_then(Value::as_i64);
                        let kind = if code == Some(429) {
                            ErrorKind::RateLimited
                        } else if code == Some(401) || code == Some(403) {
                            ErrorKind::Authentication
                        } else {
                            ErrorKind::Unavailable
                        };
                        return Err(ProviderError::new(
                            kind,
                            "Codex rejected quota request; verify login and CLI compatibility",
                        ));
                    }
                    return msg.get("result").cloned().ok_or_else(invalid);
                }
            };
            let mut send = |v: Value| -> Result<(), ProviderError> {
                serde_json::to_writer(&mut stdin, &v).map_err(|_| invalid())?;
                stdin
                    .write_all(b"\n")
                    .and_then(|_| stdin.flush())
                    .map_err(|_| {
                        ProviderError::new(ErrorKind::Network, "Codex transport write failed")
                    })
            };
            let operation = (|| {
                send(
                    json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"ai_quota_monitor","version":env!("CARGO_PKG_VERSION")}}}),
                )?;
                receive(1)?;
                send(json!({"method":"initialized"}))?;
                send(json!({"id":2,"method":"account/rateLimits/read"}))?;
                parse(&receive(2)?)
            })();
            drop(rx);
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            operation
        })();
        let _ = child.kill();
        let _ = child.wait();
        result
    }
}
