pub mod desktop;
pub mod ntfy;
pub mod policy;
pub mod runtime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    High,
    Critical,
    Exhausted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEvent {
    Threshold,
    QuotaExhausted,
    QuotaRenewed,
    QuotaRecovered,
    MonitorFailed,
    MonitorRecovered,
    Diagnostic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Notification {
    pub event: NotificationEvent,
    pub observed_at: Option<i64>,
    pub provider: String,
    pub profile: String,
    pub window: String,
    pub label: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at: Option<i64>,
    /// Exact configured consumed-percent threshold (zero for non-threshold events).
    pub threshold: f64,
    pub severity: Severity,
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryError {
    Transient {
        retry_after: Option<std::time::Duration>,
    },
    Permanent,
}

pub trait Transport: Send + 'static {
    fn name(&self) -> &'static str;
    fn send(&mut self, notification: &Notification) -> Result<(), DeliveryError>;
}
