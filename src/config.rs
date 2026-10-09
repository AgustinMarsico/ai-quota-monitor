use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderConfig {
    pub provider: String,
    pub profile: String,
    pub enabled: bool,
    pub interval_seconds: u64,
    pub codex_bin: PathBuf,
    pub codex_home: Option<PathBuf>,
    pub auth_file: Option<PathBuf>,
}
impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            provider: "codex".into(),
            profile: "default".into(),
            enabled: true,
            interval_seconds: 180,
            codex_bin: "codex".into(),
            codex_home: None,
            auth_file: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub stale_seconds: u64,
    pub max_backoff_seconds: u64,
    pub retention_days: Option<u32>,
    pub notifications: bool,
    pub compact_metric: String,
    pub compact_color: String,
    pub providers: Vec<ProviderConfig>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            stale_seconds: 600,
            max_backoff_seconds: 3600,
            retention_days: None,
            notifications: true,
            compact_metric: "remaining".into(),
            compact_color: "never".into(),
            providers: vec![
                ProviderConfig::default(),
                ProviderConfig {
                    provider: "opencode-go".into(),
                    ..ProviderConfig::default()
                },
            ],
        }
    }
}
pub fn xdg(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback)
        })
}
#[derive(Clone)]
pub struct Locations {
    pub state: PathBuf,
    pub cache: PathBuf,
    pub config: PathBuf,
}
impl Default for Locations {
    fn default() -> Self {
        Self {
            state: xdg("XDG_STATE_HOME", ".local/state").join("ai-quota"),
            cache: xdg("XDG_CACHE_HOME", ".cache").join("ai-quota/status.json"),
            config: xdg("XDG_CONFIG_HOME", ".config").join("ai-quota/config.json"),
        }
    }
}
impl Config {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let config: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Invalid configuration JSON (contents suppressed)".to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => return Err("Cannot read configuration".into()),
        };
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<(), String> {
        if !(60..=86400).contains(&self.stale_seconds)
            || !(60..=86400).contains(&self.max_backoff_seconds)
            || self
                .retention_days
                .is_some_and(|days| !(1..=3650).contains(&days))
            || !["remaining", "used"].contains(&self.compact_metric.as_str())
            || !["never", "always", "auto"].contains(&self.compact_color.as_str())
        {
            return Err("Invalid configuration setting".into());
        }
        let mut ids = std::collections::HashSet::new();
        for p in &self.providers {
            if !["codex", "opencode-go"].contains(&p.provider.as_str())
                || !(60..=3600).contains(&p.interval_seconds)
                || p.profile.is_empty()
                || p.profile.len() > 64
                || !p
                    .profile
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                || !ids.insert((&p.provider, &p.profile))
            {
                return Err("Invalid or duplicate provider profile".into());
            }
        }
        Ok(())
    }
}
