use super::{DeliveryError, Notification, Severity, Transport};
use serde::Serialize;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};

const MAX_PAYLOAD: usize = 16 * 1024;
const MAX_RESPONSE: usize = 16 * 1024;

#[derive(Clone)]
pub struct NtfyConfig {
    pub server_url: String,
    pub topic: String,
    pub token_file: Option<std::path::PathBuf>,
}

pub struct Ntfy {
    config: NtfyConfig,
    #[cfg(test)]
    allow_loopback_http: bool,
}

impl Ntfy {
    pub fn new(config: NtfyConfig) -> Self {
        Self {
            config,
            #[cfg(test)]
            allow_loopback_http: false,
        }
    }

    fn token(&self) -> Result<Option<String>, DeliveryError> {
        let Some(path) = &self.config.token_file else {
            return Ok(None);
        };
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| DeliveryError::Permanent)?;
        let metadata = file.metadata().map_err(|_| DeliveryError::Permanent)?;
        // Require a regular, single-link, owner-only file belonging to this user.
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.len() > 4096
        {
            return Err(DeliveryError::Permanent);
        }
        let mut token = String::new();
        file.take(4097)
            .read_to_string(&mut token)
            .map_err(|_| DeliveryError::Permanent)?;
        let token = token.trim();
        if token.is_empty()
            || token.len() > 4096
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
        {
            return Err(DeliveryError::Permanent);
        }
        Ok(Some(token.to_owned()))
    }
}

#[derive(Serialize)]
struct Payload<'a> {
    topic: &'a str,
    title: &'a str,
    message: String,
    priority: u8,
    tags: [&'static str; 1],
}

fn curl_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

impl Transport for Ntfy {
    fn name(&self) -> &'static str {
        "ntfy"
    }

    fn send(&mut self, notification: &Notification) -> Result<(), DeliveryError> {
        let token = self.token()?;
        let priority = match notification.severity {
            Severity::Warning => 3,
            Severity::High => 4,
            Severity::Critical | Severity::Exhausted => 5,
        };
        let tag = match notification.severity {
            Severity::Warning => "warning",
            Severity::High => "warning",
            Severity::Critical | Severity::Exhausted => "rotating_light",
        };
        let mut message = notification.message.clone();
        message.push_str(&format!(
            "\nEvent: {:?}\nObserved (UTC): {}",
            notification.event,
            notification
                .observed_at
                .map_or_else(|| "not recorded".to_string(), |at| at.to_string())
        ));
        if notification.window != "monitor" {
            message.push_str(&format!(
                "\nConsumed: {:?}%\nAvailable: {:?}%",
                notification.used_percent, notification.remaining_percent
            ));
            if let Some(reset) = notification.resets_at {
                message.push_str(&format!("\nResets at (UTC): {}", reset));
            } else {
                message.push_str("\nReset: not reported");
            }
        }
        let payload = serde_json::to_string(&Payload {
            topic: &self.config.topic,
            title: &notification.title,
            message,
            priority,
            tags: [tag],
        })
        .map_err(|_| DeliveryError::Permanent)?;
        if payload.len() > MAX_PAYLOAD {
            return Err(DeliveryError::Permanent);
        }
        let mut settings = format!("url = {}\nrequest = \"POST\"\nheader = {}\ndata = {}\noutput = \"/dev/null\"\nmax-time = 8\nconnect-timeout = 3\nmax-filesize = {}\nwrite-out = {}\n", curl_quote(&format!("{}/", self.config.server_url)), curl_quote("Content-Type: application/json"), curl_quote(&payload), MAX_RESPONSE, curl_quote("%{http_code}\\n%{header_json}"));
        if let Some(token) = token.as_deref() {
            settings.push_str(&format!(
                "header = {}\n",
                curl_quote(&format!("Authorization: Bearer {token}"))
            ));
        }
        if settings.len() > MAX_PAYLOAD + 8192 {
            return Err(DeliveryError::Permanent);
        }

        // All sensitive data is delivered over stdin, never argv. Curl runs without
        // user configuration, redirects, or retries; its own deadlines bound network I/O.
        let protocol = if self.test_http_allowed() {
            "=https,http"
        } else {
            "=https"
        };
        let mut child = Command::new("curl")
            .args([
                "-q",
                "--silent",
                "--no-location",
                "--proto",
                protocol,
                "--config",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| DeliveryError::Transient { retry_after: None })?;
        let Some(mut stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DeliveryError::Transient { retry_after: None });
        };
        let write_result = stdin.write_all(settings.as_bytes());
        drop(stdin);
        if write_result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DeliveryError::Transient { retry_after: None });
        }

        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DeliveryError::Transient { retry_after: None });
        };
        let reader = match thread::Builder::new()
            .name("quota-ntfy-response".into())
            .spawn(move || {
                use std::io::Read;
                let mut bytes = Vec::new();
                let result = stdout
                    .by_ref()
                    .take((MAX_RESPONSE + 1) as u64)
                    .read_to_end(&mut bytes);
                (result.is_ok() && bytes.len() <= MAX_RESPONSE, bytes)
            }) {
            Ok(reader) => reader,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DeliveryError::Transient { retry_after: None });
            }
        };
        let deadline = Instant::now() + Duration::from_secs(9);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let (bounded, bytes) = reader
                        .join()
                        .map_err(|_| DeliveryError::Transient { retry_after: None })?;
                    if !status.success() || !bounded {
                        return Err(DeliveryError::Transient { retry_after: None });
                    }
                    let response = String::from_utf8_lossy(&bytes);
                    let (code, headers) = response
                        .split_once('\n')
                        .ok_or(DeliveryError::Transient { retry_after: None })?;
                    let code = code
                        .trim()
                        .parse::<u16>()
                        .map_err(|_| DeliveryError::Transient { retry_after: None })?;
                    let retry_after = serde_json::from_str::<serde_json::Value>(headers.trim())
                        .ok()
                        .and_then(|json| {
                            json.get("retry-after")
                                .and_then(|value| value.as_array())
                                .and_then(|values| values.first())
                                .and_then(|value| value.as_str())
                                .and_then(parse_retry_after)
                        });
                    return match code {
                        200..=299 => Ok(()),
                        408 | 429 | 500..=599 => Err(DeliveryError::Transient { retry_after }),
                        _ => Err(DeliveryError::Permanent),
                    };
                }
                _ if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(DeliveryError::Transient { retry_after: None });
                }
                _ => thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

impl Ntfy {
    #[cfg(test)]
    fn test_http_allowed(&self) -> bool {
        self.allow_loopback_http
            && (self.config.server_url.starts_with("http://127.0.0.1:")
                || self.config.server_url.starts_with("http://[::1]:"))
    }

    #[cfg(not(test))]
    fn test_http_allowed(&self) -> bool {
        false
    }
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(SystemTime::now()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
    };

    fn test_ntfy(url: String) -> Ntfy {
        Ntfy {
            config: NtfyConfig {
                server_url: url,
                topic: "topic\" special".into(),
                token_file: None,
            },
            allow_loopback_http: true,
        }
    }

    fn notification() -> Notification {
        Notification {
            event: crate::notifications::NotificationEvent::Threshold,
            observed_at: Some(100),
            provider: "codex".into(),
            profile: "test".into(),
            window: "5h".into(),
            label: "primary".into(),
            used_percent: 96.0,
            remaining_percent: 4.0,
            resets_at: None,
            threshold: 95.0,
            severity: Severity::Critical,
            title: "title \"quoted\"".into(),
            message: "line one\nline \"two\" \\".into(),
        }
    }

    #[test]
    fn curl_config_escapes_quotes_newlines_and_backslashes() {
        assert_eq!(curl_quote("a\\b\"c\nd\r"), "\"a\\\\b\\\"c\\nd\\r\"");
    }

    #[test]
    fn token_file_accepts_regular_and_rejects_special_or_unsafe_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        std::fs::write(&path, "fixture-token\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let ntfy = Ntfy {
            config: NtfyConfig {
                server_url: "https://example.test".into(),
                topic: "x".into(),
                token_file: Some(path.clone()),
            },
            allow_loopback_http: false,
        };
        assert_eq!(ntfy.token().unwrap().as_deref(), Some("fixture-token"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(ntfy.token(), Err(DeliveryError::Permanent));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&path, "x".repeat(4097)).unwrap();
        assert_eq!(ntfy.token(), Err(DeliveryError::Permanent));
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let linked = Ntfy {
            config: NtfyConfig {
                server_url: "https://example.test".into(),
                topic: "x".into(),
                token_file: Some(link),
            },
            allow_loopback_http: false,
        };
        assert_eq!(linked.token(), Err(DeliveryError::Permanent));
    }

    #[test]
    fn fifo_token_open_does_not_wait_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token-fifo");
        let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: path_c is a valid NUL-terminated pathname for this test fixture.
        assert_eq!(unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) }, 0);
        let ntfy = Ntfy {
            config: NtfyConfig {
                server_url: "https://example.test".into(),
                topic: "x".into(),
                token_file: Some(path),
            },
            allow_loopback_http: false,
        };
        let started = Instant::now();
        assert_eq!(ntfy.token(), Err(DeliveryError::Permanent));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn real_curl_posts_escaped_config_to_loopback_only() {
        if Command::new("curl")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
                if line.to_ascii_lowercase().starts_with("content-length:") {
                    length = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            let response = b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            use std::io::Write;
            reader.get_mut().write_all(response).unwrap();
            (request, String::from_utf8(body).unwrap())
        });
        let mut ntfy = test_ntfy(format!("http://{address}"));
        assert!(ntfy.test_http_allowed());
        assert_eq!(
            ntfy.send(&notification()),
            Err(DeliveryError::Transient {
                retry_after: Some(Duration::from_secs(2))
            })
        );
        let (headers, body) = server.join().unwrap();
        assert!(headers.starts_with("POST / HTTP/1.1\r\n"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("content-type: application/json"));
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["topic"], "topic\" special");
        assert_eq!(payload["title"], "title \"quoted\"");
        assert!(payload["message"]
            .as_str()
            .unwrap()
            .contains("line \"two\""));
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(parse_retry_after("17"), Some(Duration::from_secs(17)));
        let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(5));
        assert!(parse_retry_after(&future).is_some_and(|delay| delay <= Duration::from_secs(5)));
    }
}
