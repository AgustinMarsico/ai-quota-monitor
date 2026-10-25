//! A monitor-owned Console session. Never read OpenCode's credential database or refresh tokens.
use crate::{
    config::{xdg, ProviderConfig},
    model::{now, ErrorKind, ProviderError},
    providers::{go, Reading},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const CLIENT: &str = "ai-quota-monitor";
const TOKEN_URL: &str = "https://opencode.ai/console/auth/device/token";
const CODE_URL: &str = "https://opencode.ai/console/auth/device/code";

fn auth_error(message: &str) -> ProviderError {
    ProviderError::new(ErrorKind::Authentication, message)
}
fn invalid() -> ProviderError {
    ProviderError::new(
        ErrorKind::InvalidResponse,
        "Invalid Console response (contents suppressed)",
    )
}
fn storage_error() -> ProviderError {
    auth_error("Cannot safely access monitor-owned Console credentials; see docs/opencode-go.md")
}
fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 16384
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
}

// Intentionally no Debug implementation: this structure contains secrets.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
    org_id: String,
    #[serde(default)]
    refresh_pending: bool,
}
impl Session {
    fn from_response(
        value: Value,
        timestamp: i64,
        previous_org: Option<&str>,
    ) -> Result<Self, ProviderError> {
        let text = |key| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(invalid)
        };
        let access_token = text("access_token")?;
        let refresh_token = text("refresh_token")?;
        let org_id = text("org_id")?;
        let lifetime = value
            .get("expires_in")
            .and_then(Value::as_i64)
            .filter(|n| (61..=31 * 86400).contains(n))
            .ok_or_else(invalid)?;
        if !valid_token(&access_token)
            || !valid_token(&refresh_token)
            || !valid_org(&org_id)
            || previous_org.is_some_and(|old| old != org_id)
        {
            return Err(invalid());
        }
        Ok(Self {
            access_token,
            refresh_token,
            org_id,
            expires_at: timestamp + lifetime,
            refresh_pending: false,
        })
    }
    fn needs_refresh(&self, timestamp: i64) -> bool {
        self.expires_at <= timestamp.saturating_add(60)
    }
}
fn valid_org(org: &str) -> bool {
    (org.starts_with("org_") || org.starts_with("wrk_"))
        && org.len() > 4
        && org.len() <= 256
        && org
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub fn auth_path(config: &ProviderConfig) -> PathBuf {
    config.console_auth_file.clone().unwrap_or_else(|| {
        xdg("XDG_STATE_HOME", ".local/state")
            .join("ai-quota/console-auth")
            .join(&config.profile)
            .join("credentials.json")
    })
}
fn private_metadata(file: &File) -> Result<(), ProviderError> {
    let meta = file.metadata().map_err(|_| storage_error())?;
    // SAFETY: geteuid has no preconditions.
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.permissions().mode() & 0o077 != 0
        || meta.nlink() != 1
    {
        return Err(storage_error());
    }
    Ok(())
}
fn load(path: &Path) -> Result<Session, ProviderError> {
    let file = OpenOptions::new()
        .read(true)
        // A configured FIFO without a writer must not block the daemon while it
        // holds writer.lock. Reject non-regular files after the nonblocking open.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| {
            ProviderError::new(
                ErrorKind::CredentialsMissing,
                "Monitor Console session missing; run ai-quota go-login",
            )
        })?;
    private_metadata(&file)?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| storage_error())?;
    if bytes.len() > 65536 {
        return Err(storage_error());
    }
    let session: Session = serde_json::from_slice(&bytes).map_err(|_| storage_error())?;
    if !valid_token(&session.access_token)
        || !valid_token(&session.refresh_token)
        || !valid_org(&session.org_id)
        || session.expires_at <= 0
    {
        return Err(storage_error());
    }
    Ok(session)
}
fn lock(path: &Path) -> Result<File, ProviderError> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(storage_error)?;
    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        // Never chmod an arbitrary existing credential directory.
        if !meta.is_dir()
            || meta.permissions().mode() & 0o077 != 0
            || meta.uid() != unsafe { libc::geteuid() }
        {
            return Err(storage_error());
        }
    } else {
        std::fs::create_dir_all(dir).map_err(|_| storage_error())?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| storage_error())?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("session.lock"))
        .map_err(|_| storage_error())?;
    private_metadata(&file)?;
    file.try_lock()
        .map_err(|_| auth_error("Another Console authentication operation is in progress"))?;
    Ok(file)
}
fn save(path: &Path, session: &Session) -> Result<(), ProviderError> {
    crate::storage::atomic_write(
        path,
        &serde_json::to_vec(session).map_err(|_| storage_error())?,
    )
    .map_err(|_| storage_error())
}
pub fn prerequisite(config: &ProviderConfig) -> bool {
    load(&auth_path(config)).is_ok_and(|session| !session.refresh_pending)
}

fn encode(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            result.push(b as char);
        } else {
            result.push_str(&format!("%{b:02X}"));
        }
    }
    result
}
fn request(
    url: &str,
    token: Option<&str>,
    form: &[(&str, &str)],
) -> Result<Vec<u8>, ProviderError> {
    // URLs are internal constants, never taken from configuration or token responses.
    if ![TOKEN_URL, CODE_URL, go::CONSOLE_USAGE_URL].contains(&url)
        || token.is_some_and(|s| !valid_token(s))
    {
        return Err(invalid());
    }
    let mut settings = String::new();
    if let Some(token) = token {
        settings.push_str(&format!("header = \"Authorization: Bearer {token}\"\n"));
    }
    if !form.is_empty() {
        settings.push_str("data = \"");
        settings.push_str(
            &form
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect::<Vec<_>>()
                .join("&"),
        );
        settings.push_str("\"\n");
    }
    let mut command = Command::new("curl");
    // Must be the first argument: otherwise curl can load an unsafe user curlrc.
    command.arg("-q");
    if url == go::CONSOLE_USAGE_URL {
        command.arg("--include");
    }
    let mut child = command
        .args([
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
            "--header",
            "Accept: application/json",
            "--user-agent",
            "ai-quota-monitor/0.1",
            "--write-out",
            "\n__AI_QUOTA_HTTP__%{http_code}",
            "--config",
            "-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ProviderError::new(ErrorKind::Unavailable, "Cannot start HTTPS transport"))?;
    if child
        .stdin
        .take()
        .is_none_or(|mut pipe| pipe.write_all(settings.as_bytes()).is_err())
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ProviderError::new(
            ErrorKind::Internal,
            "Secure authentication pipe failed",
        ));
    }
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .ok_or_else(invalid)?
        .take(1_100_001)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > 1_100_000 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(invalid());
    }
    let status = child.wait().map_err(|_| invalid())?;
    if !status.success() {
        return Err(ProviderError::new(
            ErrorKind::Network,
            "Console HTTPS transport failed (details suppressed)",
        ));
    }
    Ok(bytes)
}
fn json_response(bytes: &[u8]) -> Result<(u16, Value), ProviderError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let (body, code) = text
        .rsplit_once("\n__AI_QUOTA_HTTP__")
        .ok_or_else(invalid)?;
    Ok((
        code.trim().parse().map_err(|_| invalid())?,
        serde_json::from_str(body).map_err(|_| invalid())?,
    ))
}

#[derive(Debug, PartialEq)]
enum DeviceState {
    Authorized,
    Pending,
    SlowDown,
}
fn device_state(status: u16, value: &Value) -> Result<DeviceState, ProviderError> {
    match (status, value.get("error").and_then(Value::as_str)) {
        (200, _) => Ok(DeviceState::Authorized),
        (400, Some("authorization_pending")) => Ok(DeviceState::Pending),
        (400, Some("slow_down")) => Ok(DeviceState::SlowDown),
        _ => Err(auth_error(
            "Device authorization denied, expired, or failed (response suppressed)",
        )),
    }
}

/// Diagnostic mode never refreshes or writes a credential, cache, or lock file.
pub fn fetch(config: &ProviderConfig, read_only: bool) -> Result<Reading, ProviderError> {
    fetch_with(config, read_only, request)
}
fn fetch_with(
    config: &ProviderConfig,
    read_only: bool,
    mut transport: impl FnMut(&str, Option<&str>, &[(&str, &str)]) -> Result<Vec<u8>, ProviderError>,
) -> Result<Reading, ProviderError> {
    let path = auth_path(config);
    let _guard = if read_only { None } else { Some(lock(&path)?) };
    let mut session = load(&path)?;
    if session.refresh_pending {
        return Err(auth_error("Previous refresh outcome uncertain; run ai-quota go-login (old refresh token will not be reused)"));
    }
    if session.needs_refresh(now()) {
        if read_only {
            return Err(auth_error("Console token expired or expires shortly; diagnostic check cannot renew credentials"));
        }
        // Persist BEFORE contacting Console. If killed, disconnected, or disk-full after rotation,
        // subsequent invocations must never replay this one-use refresh token.
        session.refresh_pending = true;
        save(&path, &session)?;
        let started = now();
        let (status, value) = json_response(&transport(
            TOKEN_URL,
            None,
            &[
                ("client_id", CLIENT),
                ("grant_type", "refresh_token"),
                ("refresh_token", &session.refresh_token),
            ],
        )?)?;
        if status != 200 {
            return Err(auth_error(
                "Console refresh failed; run ai-quota go-login; no automatic replay",
            ));
        }
        session = Session::from_response(value, started, Some(&session.org_id))?;
        save(&path, &session)?;
    }
    // The documented org-scoped device grant does not require x-org-id.
    let bytes = transport(go::CONSOLE_USAGE_URL, Some(&session.access_token), &[])?;
    go::console_response(&bytes, now())
}

/// Explicit, user-initiated login only. No browser cookie import and no automatic browser launch.
pub fn login(config: &ProviderConfig) -> Result<(), ProviderError> {
    let path = auth_path(config);
    let _guard = lock(&path)?;
    // An existing session is intentionally not overwritten, including a pending-refresh marker.
    if path.exists() {
        return Err(auth_error("Session file already exists; move it to a private backup before authorizing a replacement"));
    }
    let (status, device) = json_response(&request(
        CODE_URL,
        None,
        &[("client_id", CLIENT), ("supports_org_scope", "true")],
    )?)?;
    if status != 200 {
        return Err(auth_error("Console device authorization unavailable"));
    }
    let device_code = device
        .get("device_code")
        .and_then(Value::as_str)
        .filter(|s| valid_token(s))
        .ok_or_else(invalid)?;
    let uri = device
        .get("verification_uri_complete")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let uri = if uri.starts_with("/console/device?") {
        format!("https://opencode.ai{uri}")
    } else {
        uri.into()
    };
    if !uri.starts_with("https://opencode.ai/console/device?")
        || uri.bytes().any(|b| b.is_ascii_control())
    {
        return Err(invalid());
    }
    let expires = device
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|n| (1..=600).contains(n))
        .ok_or_else(invalid)?;
    let mut interval = device
        .get("interval")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(5, 60);
    println!("Authorize the monitor's own Console session (choose your Go workspace):\n{uri}");
    let deadline = Instant::now() + Duration::from_secs(expires);
    loop {
        if Instant::now() + Duration::from_secs(interval) >= deadline {
            return Err(auth_error("Device authorization expired"));
        }
        std::thread::sleep(Duration::from_secs(interval));
        let started = now();
        let (status, value) = json_response(&request(
            TOKEN_URL,
            None,
            &[
                ("client_id", CLIENT),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ],
        )?)?;
        match device_state(status, &value)? {
            DeviceState::Authorized => {
                let session = Session::from_response(value, started, None)?;
                // Authentication does not imply quota endpoint compatibility. Verify separately
                // with `check`, retaining useful error state if this undocumented API changes.
                save(&path, &session)?;
                println!(
                    "Monitor session saved privately. Run ai-quota check --provider opencode-go."
                );
                return Ok(());
            }
            DeviceState::Pending => (),
            DeviceState::SlowDown => interval = interval.saturating_add(5).min(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn token() -> Value {
        json!({"access_token":"fixture-access", "refresh_token":"fixture-refresh", "expires_in":3600, "org_id":"org_fixture"})
    }
    #[test]
    fn expiry_scope_and_tokens() {
        let session = Session::from_response(token(), 1000, None).unwrap();
        assert!(!session.needs_refresh(4539));
        assert!(session.needs_refresh(4540));
        assert!(session.needs_refresh(4601));
        assert!(Session::from_response(token(), 1000, Some("org_other")).is_err());
        for field in ["access_token", "refresh_token", "org_id", "expires_in"] {
            let mut value = token();
            value.as_object_mut().unwrap().remove(field);
            assert!(Session::from_response(value, 1000, None).is_err());
        }
        for bad in ["", "token\nInjected", "token\"", "token\\"] {
            assert!(!valid_token(bad));
        }
        assert_eq!(encode("x+y/&=\n"), "x%2By%2F%26%3D%0A");
    }
    #[test]
    fn device_auth_pending_denial_expiry_and_http_failures() {
        assert_eq!(
            device_state(400, &json!({"error":"authorization_pending"})).unwrap(),
            DeviceState::Pending
        );
        assert_eq!(
            device_state(400, &json!({"error":"slow_down"})).unwrap(),
            DeviceState::SlowDown
        );
        assert_eq!(
            device_state(200, &token()).unwrap(),
            DeviceState::Authorized
        );
        for (status, error) in [
            (400, "access_denied"),
            (400, "expired_token"),
            (401, "unauthorized"),
            (403, "forbidden"),
            (429, "slow_down"),
            (500, "server_error"),
        ] {
            let result = device_state(
                status,
                &json!({"error":error,"refresh_token":"secret-fixture"}),
            )
            .unwrap_err();
            assert_eq!(result.kind, ErrorKind::Authentication);
            assert!(!result.message.contains("secret-fixture"));
        }
    }
    #[test]
    fn private_storage_and_locking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth/credentials.json");
        let guard = lock(&path).unwrap();
        assert!(lock(&path).is_err());
        let mut session = Session::from_response(token(), 1000, None).unwrap();
        save(&path, &session).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(load(&path).unwrap().org_id, "org_fixture");
        session.refresh_pending = true;
        save(&path, &session).unwrap();
        assert!(load(&path).unwrap().refresh_pending);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&path).is_err());
        drop(guard);
        assert!(lock(&path).is_ok());
    }
    #[test]
    fn no_symlink_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("real.json");
        save(&path, &Session::from_response(token(), 1000, None).unwrap()).unwrap();
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(load(&link).is_err());
    }
    #[test]
    fn fifo_credential_is_rejected_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.fifo");
        let pathname = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: pathname is a valid, NUL-terminated path in an isolated fixture.
        assert_eq!(unsafe { libc::mkfifo(pathname.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(load(&path).is_err());
        for diagnostic in [true, false] {
            assert!(fetch_with(&config(path.clone()), diagnostic, |_, _, _| {
                panic!("invalid credential must not initiate a request")
            })
            .is_err());
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
    fn config(path: PathBuf) -> ProviderConfig {
        ProviderConfig {
            console_auth_file: Some(path),
            ..Default::default()
        }
    }
    #[test]
    fn diagnostic_is_read_only_even_with_expired_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        save(&path, &Session::from_response(token(), 1000, None).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(fetch_with(&config(path.clone()), true, |_, _, _| panic!(
            "must not contact OAuth"
        ))
        .is_err());
        assert_eq!(before, std::fs::read(path).unwrap());
        assert!(!dir.path().join("session.lock").exists());
    }
    #[test]
    fn rotation_is_saved_before_usage_and_org_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        save(&path, &Session::from_response(token(), 1000, None).unwrap()).unwrap();
        let mut calls = 0;
        let result = fetch_with(&config(path.clone()), false, |url, bearer, form| {
            calls += 1;
            if calls == 1 {
                assert_eq!(url, TOKEN_URL);
                assert!(bearer.is_none());
                assert!(form.contains(&("client_id", CLIENT)));
                assert!(load(&path).unwrap().refresh_pending);
                let mut fresh = token();
                fresh["access_token"] = "new-access".into();
                fresh["refresh_token"] = "new-refresh".into();
                Ok(format!("{fresh}\n__AI_QUOTA_HTTP__200").into_bytes())
            } else {
                assert_eq!(url, go::CONSOLE_USAGE_URL);
                assert_eq!(bearer, Some("new-access"));
                assert!(form.is_empty());
                assert!(!load(&path).unwrap().refresh_pending);
                assert_eq!(load(&path).unwrap().refresh_token, "new-refresh");
                Ok(b"{\"access\":{\"meters\":{\"fiveHour\":{\"usedMicroCents\":1,\"limitMicroCents\":100}}}}\n__AI_QUOTA_HTTP__200".to_vec())
            }
        }).unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result.quotas[0].remaining_percent, 99.0);
    }
    #[test]
    fn failed_refresh_is_never_replayed() {
        for wire in [
            b"{\"error\":\"invalid_grant\"}\n__AI_QUOTA_HTTP__400".as_slice(),
            b"{\"error\":\"busy\"}\n__AI_QUOTA_HTTP__429",
            b"not json\n__AI_QUOTA_HTTP__500",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.json");
            save(&path, &Session::from_response(token(), 1000, None).unwrap()).unwrap();
            let config = config(path.clone());
            assert!(fetch_with(&config, false, |_, _, _| Ok(wire.to_vec())).is_err());
            assert!(load(&path).unwrap().refresh_pending);
            assert!(fetch_with(&config, false, |_, _, _| panic!("must not replay")).is_err());
        }
    }
    #[test]
    fn fresh_diagnostic_uses_only_access_token_and_retains_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        save(
            &path,
            &Session::from_response(token(), now(), None).unwrap(),
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let error = fetch_with(&config(path.clone()), true, |url, bearer, form| {
            assert_eq!(url, go::CONSOLE_USAGE_URL);
            assert_eq!(bearer, Some("fixture-access"));
            assert!(form.is_empty());
            Ok(b"{\"secret\":\"must not escape\"}\n__AI_QUOTA_HTTP__403".to_vec())
        })
        .err()
        .unwrap();
        assert_eq!(error.kind, ErrorKind::Authentication);
        assert_eq!(before, std::fs::read(path).unwrap());
        assert!(!dir.path().join("session.lock").exists());
    }
}
