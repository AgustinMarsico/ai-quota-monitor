use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u32 = 1;
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DataState {
    Fresh,
    Stale,
    Unavailable,
    Error,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Authentication,
    CredentialsMissing,
    RateLimited,
    Network,
    InvalidResponse,
    Unavailable,
    Internal,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub message: String,
    pub retry_after_seconds: Option<u64>,
}
impl ProviderError {
    pub fn new(kind: ErrorKind, message: &str) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after_seconds: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quota {
    pub window: String,
    pub label: String,
    pub duration_minutes: Option<u64>,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at: Option<i64>,
}
impl Quota {
    pub fn new(
        window: &str,
        label: String,
        duration_minutes: Option<u64>,
        used: f64,
        resets_at: Option<i64>,
    ) -> Result<Self, ProviderError> {
        if !used.is_finite()
            || !(0.0..=100.0).contains(&used)
            || duration_minutes == Some(0)
            || resets_at.is_some_and(|t| t <= 0)
        {
            return Err(ProviderError::new(
                ErrorKind::InvalidResponse,
                "Invalid quota values",
            ));
        }
        Ok(Self {
            window: window.into(),
            label,
            duration_minutes,
            used_percent: used,
            remaining_percent: 100.0 - used,
            resets_at,
        })
    }
}
pub fn duration_label(minutes: u64) -> String {
    if minutes.is_multiple_of(1440) {
        format!("{}d", minutes / 1440)
    } else if minutes.is_multiple_of(60) {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub provider: String,
    pub profile: String,
    pub account: Option<String>,
    pub model: Option<String>,
    pub plan: Option<String>,
    pub source: String,
    pub limitations: Vec<String>,
    pub quotas: Vec<Quota>,
    pub last_success_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub state: DataState,
    pub error: Option<ProviderError>,
    pub consecutive_failures: u32,
    pub next_attempt_at: i64,
}
impl ProviderStatus {
    pub fn empty(provider: &str, profile: &str) -> Self {
        Self {
            provider: provider.into(),
            profile: profile.into(),
            account: None,
            model: None,
            plan: None,
            source: String::new(),
            limitations: vec![],
            quotas: vec![],
            last_success_at: None,
            last_attempt_at: None,
            state: DataState::Unavailable,
            error: None,
            consecutive_failures: 0,
            next_attempt_at: 0,
        }
    }
    pub fn age_state(&mut self, timestamp: i64, stale_seconds: u64) {
        self.state = if self.quotas.is_empty() {
            if self.error.is_some() {
                DataState::Error
            } else {
                DataState::Unavailable
            }
        } else if self.error.is_some()
            || self
                .last_success_at
                .is_none_or(|t| timestamp - t > stale_seconds as i64 || t > timestamp + 60)
            || self
                .quotas
                .iter()
                .any(|q| q.resets_at.is_some_and(|t| t <= timestamp))
        {
            DataState::Stale
        } else {
            DataState::Fresh
        };
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub generated_at: i64,
    pub stale_seconds: u64,
    pub providers: Vec<ProviderStatus>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err("Unsupported cache schema".into());
        }
        let mut identities = std::collections::HashSet::new();
        for p in &self.providers {
            if p.last_success_at.is_some_and(|t| t <= 0)
                || p.last_attempt_at.is_some_and(|t| t <= 0)
                || p.next_attempt_at < 0
            {
                return Err("Invalid cache timestamp".into());
            }
            if !identities.insert((&p.provider, &p.profile)) {
                return Err("Duplicate profile in cache".into());
            }
            let mut windows = std::collections::HashSet::new();
            for q in &p.quotas {
                if !windows.insert(&q.window)
                    || !q.used_percent.is_finite()
                    || !q.remaining_percent.is_finite()
                    || !(0.0..=100.0).contains(&q.used_percent)
                    || !(0.0..=100.0).contains(&q.remaining_percent)
                    || (q.used_percent + q.remaining_percent - 100.0).abs() > 0.000001
                    || q.duration_minutes == Some(0)
                    || q.resets_at.is_some_and(|t| t <= 0)
                {
                    return Err("Invalid normalized cache quota".into());
                }
            }
        }
        Ok(())
    }
    pub fn aged(mut self, timestamp: i64) -> Self {
        for p in &mut self.providers {
            p.age_state(timestamp, self.stale_seconds);
        }
        self
    }
}
