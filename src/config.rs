use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoAuth {
    #[default]
    ApiKey,
    ConsoleOauth,
}

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
    pub go_auth: GoAuth,
    pub console_auth_file: Option<PathBuf>,
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
            go_auth: GoAuth::ApiKey,
            console_auth_file: None,
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
    #[serde(default)]
    pub notification_transports: NotificationTransportsConfig,
    /// None keeps the original 80/90/95 notification behavior.
    pub notification_policy: Option<NotificationPolicyConfig>,
    pub compact_metric: String,
    pub compact_color: String,
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationTransportsConfig {
    pub desktop: bool,
    pub ntfy: Option<NtfyConfig>,
}
impl Default for NotificationTransportsConfig {
    fn default() -> Self {
        Self {
            desktop: true,
            ntfy: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtfyConfig {
    pub enabled: bool,
    pub server_url: String,
    pub topic: String,
    pub token_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationPolicyConfig {
    pub thresholds_used_percent: Vec<f64>,
    pub events: NotificationEventsConfig,
    pub recovery: NotificationRecoveryConfig,
    pub monitor_failure: NotificationMonitorFailureConfig,
    pub overrides: Vec<NotificationPolicyOverride>,
}

impl Default for NotificationPolicyConfig {
    fn default() -> Self {
        Self {
            thresholds_used_percent: vec![80.0, 90.0, 95.0, 100.0],
            events: NotificationEventsConfig::default(),
            recovery: NotificationRecoveryConfig::default(),
            monitor_failure: NotificationMonitorFailureConfig::default(),
            overrides: Vec::new(),
        }
    }
}

impl NotificationPolicyConfig {
    /// Exact-match precedence: global, provider, profile, then window.
    pub fn thresholds_for(&self, provider: &str, profile: &str, window: &str) -> &[f64] {
        let mut selected = &self.thresholds_used_percent;
        let mut specificity = 0;
        for override_config in &self.overrides {
            if override_config.provider != provider
                || override_config
                    .profile
                    .as_deref()
                    .is_some_and(|p| p != profile)
                || override_config
                    .window
                    .as_deref()
                    .is_some_and(|w| w != window)
            {
                continue;
            }
            let level = 1
                + u8::from(override_config.profile.is_some())
                + 2 * u8::from(override_config.window.is_some());
            if level > specificity {
                selected = &override_config.thresholds_used_percent;
                specificity = level;
            }
        }
        selected
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationEventsConfig {
    pub threshold: bool,
    pub quota_renewed: bool,
    pub quota_recovered: bool,
    pub monitor_failed: bool,
    pub monitor_recovered: bool,
}

impl Default for NotificationEventsConfig {
    fn default() -> Self {
        Self {
            threshold: true,
            quota_renewed: true,
            quota_recovered: true,
            monitor_failed: false,
            monitor_recovered: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationRecoveryConfig {
    pub margin_percent: f64,
    pub confirmations: u32,
}

impl Default for NotificationRecoveryConfig {
    fn default() -> Self {
        Self {
            margin_percent: 5.0,
            confirmations: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationMonitorFailureConfig {
    pub consecutive_attempts: u32,
    pub minimum_duration_seconds: u64,
}

impl Default for NotificationMonitorFailureConfig {
    fn default() -> Self {
        Self {
            consecutive_attempts: 3,
            minimum_duration_seconds: 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationPolicyOverride {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    pub thresholds_used_percent: Vec<f64>,
}

fn valid_thresholds(thresholds: &[f64]) -> bool {
    !thresholds.is_empty()
        && thresholds.len() <= 100
        && thresholds
            .iter()
            .all(|value| value.is_finite() && *value > 0.0 && *value <= 100.0)
        && thresholds.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_policy(policy: &NotificationPolicyConfig, providers: &[ProviderConfig]) -> bool {
    if !valid_thresholds(&policy.thresholds_used_percent)
        || !policy.recovery.margin_percent.is_finite()
        || policy.recovery.margin_percent <= 0.0
        || policy.recovery.margin_percent > 100.0
        || !(2..=100).contains(&policy.recovery.confirmations)
        || !(1..=100).contains(&policy.monitor_failure.consecutive_attempts)
        || policy.monitor_failure.minimum_duration_seconds > 86400
        || policy.overrides.len() > 256
    {
        return false;
    }
    let mut seen = std::collections::HashSet::new();
    policy.overrides.iter().all(|rule| {
        let valid_window = match (rule.provider.as_str(), rule.window.as_deref()) {
            ("codex", Some(window)) => ["primary", "secondary"].contains(&window),
            ("opencode-go", Some(window)) => ["rolling", "weekly", "monthly"].contains(&window),
            (_, None) => true,
            _ => false,
        };
        crate::providers::PROVIDER_IDS.contains(&rule.provider.as_str())
            && valid_window
            && rule.profile.as_ref().is_none_or(|profile| {
                providers.iter().any(|configured| {
                    configured.provider == rule.provider && configured.profile == *profile
                })
            })
            && valid_thresholds(&rule.thresholds_used_percent)
            && seen.insert((&rule.provider, &rule.profile, &rule.window))
    })
}
impl Default for Config {
    fn default() -> Self {
        Self {
            stale_seconds: 600,
            max_backoff_seconds: 3600,
            retention_days: None,
            notifications: true,
            notification_transports: NotificationTransportsConfig {
                desktop: true,
                ntfy: None,
            },
            notification_policy: None,
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
        if let Some(ntfy) = &self.notification_transports.ntfy {
            let parsed = ntfy.server_url.strip_prefix("https://").filter(|rest| {
                !rest.is_empty()
                    && rest
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
            });
            if ntfy.enabled && parsed.is_none()
                || ntfy.topic.is_empty()
                || ntfy.topic.len() > 64
                || !ntfy
                    .topic
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err("Invalid notification transport configuration".into());
            }
        }
        if self
            .notification_policy
            .as_ref()
            .is_some_and(|policy| !valid_policy(policy, &self.providers))
        {
            return Err("Invalid notification policy configuration".into());
        }
        let mut ids = std::collections::HashSet::new();
        for p in &self.providers {
            if !crate::providers::PROVIDER_IDS.contains(&p.provider.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(json: &str) -> Result<Config, String> {
        let config: Config = serde_json::from_str(json).map_err(|e| e.to_string())?;
        config.validate()?;
        Ok(config)
    }

    #[test]
    fn legacy_and_policy_defaults() {
        let legacy = configured("{}").unwrap();
        assert!(legacy.notification_policy.is_none());
        let policy = configured(r#"{"notification_policy": {}}"#)
            .unwrap()
            .notification_policy
            .unwrap();
        assert_eq!(policy.thresholds_used_percent, [80.0, 90.0, 95.0, 100.0]);
        assert!(policy.events.quota_recovered);
        assert!(!policy.events.monitor_failed);
    }

    #[test]
    fn override_precedence_is_independent_of_json_order() {
        let policy = configured(
            r#"{"notification_policy":{"thresholds_used_percent":[80,90,95,100],
                "overrides":[
                    {"provider":"codex","profile":"default","window":"primary","thresholds_used_percent":[99]},
                    {"provider":"codex","window":"primary","thresholds_used_percent":[97]},
                    {"provider":"codex","thresholds_used_percent":[85]},
                    {"provider":"codex","profile":"default","thresholds_used_percent":[90]}
                ]}}"#,
        )
        .unwrap()
        .notification_policy
        .unwrap();
        assert_eq!(policy.thresholds_for("codex", "default", "primary"), [99.0]);
        assert_eq!(
            policy.thresholds_for("codex", "default", "secondary"),
            [90.0]
        );
        assert_eq!(policy.thresholds_for("codex", "other", "primary"), [97.0]);
        assert_eq!(policy.thresholds_for("codex", "other", "secondary"), [85.0]);
        assert_eq!(
            policy.thresholds_for("opencode-go", "default", "weekly"),
            [80.0, 90.0, 95.0, 100.0]
        );
    }

    #[test]
    fn rejects_invalid_policies() {
        for json in [
            r#"{"notification_policy":{"thresholds_used_percent":[]}}"#,
            r#"{"notification_policy":{"thresholds_used_percent":[90,80]}}"#,
            r#"{"notification_policy":{"thresholds_used_percent":[90,90]}}"#,
            r#"{"notification_policy":{"thresholds_used_percent":[0,100]}}"#,
            r#"{"notification_policy":{"events":{"unexpected":true}}}"#,
            r#"{"notification_policy":{"recovery":{"confirmations":0}}}"#,
            r#"{"notification_policy":{"recovery":{"confirmations":1}}}"#,
            r#"{"notification_policy":{"recovery":{"margin_percent":0}}}"#,
            r#"{"notification_policy":{"monitor_failure":{"consecutive_attempts":0}}}"#,
            r#"{"notification_policy":{"overrides":[{"provider":"unknown","thresholds_used_percent":[95]}]}}"#,
            r#"{"notification_policy":{"overrides":[{"provider":"codex","window":"","thresholds_used_percent":[95]}]}}"#,
            r#"{"notification_policy":{"overrides":[{"provider":"codex","profile":"missing","thresholds_used_percent":[95]}]}}"#,
            r#"{"notification_policy":{"overrides":[{"provider":"codex","profile":"default","window":"monthly","thresholds_used_percent":[95]}]}}"#,
            r#"{"notification_policy":{"overrides":[{"provider":"codex","thresholds_used_percent":[95]},{"provider":"codex","thresholds_used_percent":[90]}]}}"#,
        ] {
            assert!(configured(json).is_err(), "accepted invalid policy: {json}");
        }
    }
}
