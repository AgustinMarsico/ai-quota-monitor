pub mod codex;
pub mod go;
pub const PROVIDER_IDS: &[&str] = &["codex", "opencode-go"];
use crate::{
    config::ProviderConfig,
    model::{ProviderError, Quota},
};

pub struct Reading {
    pub account: Option<String>,
    pub model: Option<String>,
    pub quotas: Vec<Quota>,
    pub plan: Option<String>,
    pub source: String,
    pub limitations: Vec<String>,
}
pub trait Provider {
    fn fetch(&self, config: &ProviderConfig, timestamp: i64) -> Result<Reading, ProviderError>;
}
pub fn fetch(config: &ProviderConfig, timestamp: i64) -> Result<Reading, ProviderError> {
    match config.provider.as_str() {
        "codex" => codex::Codex.fetch(config, timestamp),
        "opencode-go" => go::Go.fetch(config, timestamp),
        _ => Err(ProviderError::new(
            crate::model::ErrorKind::Unavailable,
            "Unsupported provider",
        )),
    }
}
