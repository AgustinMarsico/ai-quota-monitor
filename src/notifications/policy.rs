use super::{Notification, NotificationEvent, Severity};
use crate::config::NotificationPolicyConfig;
use crate::model::Quota;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct AlertState {
    pub reset_at: Option<i64>,
    /// Legacy database levels: 5, 10, 20, 100 (remaining percentage).
    pub level: u8,
}

pub fn evaluate(
    provider: &str,
    profile: &str,
    quota: &Quota,
    previous: Option<AlertState>,
) -> (Option<Notification>, AlertState) {
    let level = if quota.remaining_percent <= 5.0 {
        5
    } else if quota.remaining_percent <= 10.0 {
        10
    } else if quota.remaining_percent <= 20.0 {
        20
    } else {
        100
    };
    let new_period = previous.as_ref().is_some_and(|state| {
        matches!((state.reset_at, quota.resets_at), (Some(a), Some(b)) if a.abs_diff(b) > 300)
    });
    let old_level = previous.as_ref().map_or(100, |state| state.level);
    let notification = if level < 100 && (new_period || level < old_level) {
        let threshold = 100 - level;
        let severity = if level <= 5 {
            Severity::Critical
        } else if level <= 10 {
            Severity::High
        } else {
            Severity::Warning
        };
        Some(Notification {
            event: NotificationEvent::Threshold,
            observed_at: None,
            provider: provider.to_owned(),
            profile: profile.to_owned(),
            window: quota.window.clone(),
            label: quota.label.clone(),
            used_percent: quota.used_percent,
            remaining_percent: quota.remaining_percent,
            resets_at: quota.resets_at,
            threshold: f64::from(threshold),
            severity,
            title: format!("{} quota alert", provider),
            message: format!(
                "{} [{}] {}: {:.1}% remaining",
                provider, profile, quota.label, quota.remaining_percent
            ),
        })
    } else {
        None
    };

    // Preserve the original conservative re-arming semantics exactly.
    let saved_level = if !new_period && quota.resets_at.is_some() {
        old_level.min(level)
    } else {
        level
    };
    let reset_at = if !new_period {
        previous
            .and_then(|state| state.reset_at)
            .or(quota.resets_at)
    } else {
        quota.resets_at
    };
    (
        notification,
        AlertState {
            reset_at,
            level: saved_level,
        },
    )
}

/// Durable, per-(provider, profile, window, duration_minutes) policy state.
/// Store this as JSON in the same transaction as the quota sample and alert decision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyState {
    pub version: u8,
    pub generation: u64,
    pub reset_anchor: Option<i64>,
    pub last_observed_at: Option<i64>,
    pub last_used_percent: Option<f64>,
    pub covered_used_percent: Option<f64>,
    pub limited: bool,
    pub recovery_candidate_at: Option<i64>,
    pub recovery_confirmations: u32,
    pub thresholds_used_percent: Vec<f64>,
}

impl Default for PolicyState {
    fn default() -> Self {
        Self {
            version: 1,
            generation: 0,
            reset_anchor: None,
            last_observed_at: None,
            last_used_percent: None,
            covered_used_percent: None,
            limited: false,
            recovery_candidate_at: None,
            recovery_confirmations: 0,
            thresholds_used_percent: vec![],
        }
    }
}

impl PolicyState {
    /// Import a legacy remaining-percent watermark without replaying old levels.
    pub fn from_legacy(state: AlertState) -> Self {
        let covered = (state.level < 100).then(|| f64::from(100 - state.level));
        Self {
            reset_anchor: state.reset_at,
            covered_used_percent: covered,
            limited: covered.is_some(),
            // Rebaseline on the first configured observation (no historical replay).
            thresholds_used_percent: vec![],
            ..Self::default()
        }
    }

    /// An unsuccessful request or missing window cannot confirm recovery.
    pub fn interrupt_recovery(&mut self) {
        self.recovery_candidate_at = None;
        self.recovery_confirmations = 0;
    }
}

/// Select exact overrides by specificity, never by display label. A profile
/// override must include its provider; a window override is the most specific.
pub fn effective_thresholds<'a>(
    config: &'a NotificationPolicyConfig,
    provider: &str,
    profile: &str,
    window: &str,
) -> &'a [f64] {
    let mut selected = &config.thresholds_used_percent;
    let mut specificity = 0;
    for rule in &config.overrides {
        if rule.provider != provider
            || rule
                .profile
                .as_deref()
                .is_some_and(|value| value != profile)
            || rule.window.as_deref().is_some_and(|value| value != window)
        {
            continue;
        }
        let rank = 1 + u8::from(rule.profile.is_some()) + 2 * u8::from(rule.window.is_some());
        if rank > specificity {
            selected = &rule.thresholds_used_percent;
            specificity = rank;
        }
    }
    selected
}

fn quota_notification(
    provider: &str,
    profile: &str,
    quota: &Quota,
    detail: (f64, String, String, NotificationEvent),
    observed_at: i64,
) -> Notification {
    let (threshold, title, message, event) = detail;
    Notification {
        event,
        observed_at: Some(observed_at),
        provider: provider.into(),
        profile: profile.into(),
        window: quota.window.clone(),
        label: quota.label.clone(),
        used_percent: quota.used_percent,
        remaining_percent: quota.remaining_percent,
        resets_at: quota.resets_at,
        threshold,
        severity: if threshold == 100.0 {
            Severity::Exhausted
        } else if threshold >= 95.0 {
            Severity::Critical
        } else if threshold >= 90.0 {
            Severity::High
        } else {
            Severity::Warning
        },
        title,
        message,
    }
}

/// Call only for a fresh successful observation; persist the returned state
/// even when delivery is disabled. At most one quota notification per reading.
pub fn evaluate_configured(
    provider: &str,
    profile: &str,
    quota: &Quota,
    previous: Option<PolicyState>,
    config: &NotificationPolicyConfig,
    observed_at: i64,
) -> (Vec<Notification>, PolicyState) {
    let thresholds = effective_thresholds(config, provider, profile, &quota.window);
    let had_previous = previous.is_some();
    let mut state = previous.unwrap_or_default();
    let mut emitted = Vec::new();
    if state
        .last_observed_at
        .is_some_and(|last| observed_at <= last)
    {
        return (emitted, state);
    }
    // An anchor moving backward, a relative reset jitter, or an expiry by
    // itself is not evidence of a new period.
    let renewed = state
        .reset_anchor
        .zip(quota.resets_at)
        .is_some_and(|(old, new)| {
            old <= observed_at && new > observed_at && new > old.saturating_add(300)
        });
    if renewed {
        state.generation = state.generation.saturating_add(1);
        state.reset_anchor = quota.resets_at;
        state.covered_used_percent = None;
        state.interrupt_recovery();
        let restored = thresholds
            .first()
            .is_some_and(|minimum| quota.used_percent <= minimum - config.recovery.margin_percent);
        if state.limited && restored && config.events.quota_renewed {
            emitted.push(quota_notification(
                provider,
                profile,
                quota,
                (
                    0.0,
                    format!("{provider} quota renewed"),
                    format!(
                        "{provider} [{profile}] {}: renewal observed, {:.1}% remaining",
                        quota.label, quota.remaining_percent
                    ),
                    NotificationEvent::QuotaRenewed,
                ),
                observed_at,
            ));
        }
        state.limited = false;
    } else if state.reset_anchor.is_none() {
        state.reset_anchor = quota.resets_at;
    } else if state
        .reset_anchor
        .zip(quota.resets_at)
        .is_some_and(|(old, new)| old > observed_at && new > old.saturating_add(300))
    {
        // A reset timestamp corrected before the old period ended is not a
        // renewal. Adopt its new anchor so expiry cannot reclassify it later.
        state.reset_anchor = quota.resets_at;
    }

    let changed = had_previous && !renewed && state.thresholds_used_percent != thresholds;
    if changed {
        // Rule edits never create events on their own. Preserve the old high
        // watermark and cover the current reading under the new rules.
        if let Some(current) = thresholds
            .iter()
            .copied()
            .rfind(|t| quota.used_percent >= *t)
        {
            state.covered_used_percent =
                Some(state.covered_used_percent.unwrap_or(0.0).max(current));
            state.limited = true;
        }
    }
    state.thresholds_used_percent = thresholds.to_vec();
    // Renewal alerts are allowed under the new generation, except on a rule
    // change where the current reading must be rebaselined instead.
    if !changed && config.events.threshold {
        if let Some(threshold) = thresholds.iter().copied().rfind(|t| {
            quota.used_percent >= *t
                && state
                    .covered_used_percent
                    .is_none_or(|covered| *t > covered)
        }) {
            emitted.push(quota_notification(
                provider,
                profile,
                quota,
                (
                    threshold,
                    if threshold == 100.0 {
                        format!("{provider} quota exhausted")
                    } else {
                        format!("{provider} quota alert")
                    },
                    format!(
                        "{provider} [{profile}] {}: {:.1}% remaining (observed)",
                        quota.label, quota.remaining_percent
                    ),
                    if threshold == 100.0 {
                        NotificationEvent::QuotaExhausted
                    } else {
                        NotificationEvent::Threshold
                    },
                ),
                observed_at,
            ));
        }
    }
    // Mark even disabled events as covered so re-enabling does not replay them.
    if let Some(threshold) = thresholds
        .iter()
        .copied()
        .rfind(|t| quota.used_percent >= *t)
    {
        state.covered_used_percent = Some(state.covered_used_percent.unwrap_or(0.0).max(threshold));
        state.limited = true;
    }

    let low = state.limited
        && thresholds
            .first()
            .is_some_and(|minimum| quota.used_percent <= minimum - config.recovery.margin_percent);
    if !renewed && low {
        if state.recovery_candidate_at.is_none() {
            state.recovery_candidate_at = Some(observed_at);
            state.recovery_confirmations = 1;
        } else {
            state.recovery_confirmations = state.recovery_confirmations.saturating_add(1);
        }
        if state.recovery_confirmations >= config.recovery.confirmations.max(2) {
            if config.events.quota_recovered {
                emitted.push(quota_notification(
                    provider, profile, quota, (0.0,
                    format!("{provider} quota recovered"),
                    format!("{provider} [{profile}] {}: quota margin recovery observed, {:.1}% remaining", quota.label, quota.remaining_percent),
                    NotificationEvent::QuotaRecovered),
                    observed_at,
                ));
            }
            state.generation = state.generation.saturating_add(1);
            state.covered_used_percent = None;
            state.limited = false;
            state.interrupt_recovery();
        }
    } else {
        state.interrupt_recovery();
    }
    state.last_observed_at = Some(observed_at);
    state.last_used_percent = Some(quota.used_percent);
    (emitted, state)
}

/// Persist per provider/profile; call only after a real attempted request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorState {
    pub version: u8,
    pub incident_started_at: Option<i64>,
    pub attempts: u32,
    pub notified: bool,
    #[serde(default)]
    pub announced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorOutcome {
    Failed,
    Succeeded,
    AuthChanged,
}

pub fn evaluate_monitor(
    provider: &str,
    profile: &str,
    outcome: MonitorOutcome,
    previous: Option<MonitorState>,
    config: &NotificationPolicyConfig,
    at: i64,
) -> (Option<Notification>, MonitorState) {
    let mut state = previous.unwrap_or(MonitorState {
        version: 1,
        ..MonitorState::default()
    });
    let event = match outcome {
        MonitorOutcome::AuthChanged => {
            state.incident_started_at = None;
            state.attempts = 0;
            state.notified = false;
            state.announced = false;
            None
        }
        MonitorOutcome::Succeeded => {
            let was_notified = state.announced;
            state.incident_started_at = None;
            state.attempts = 0;
            state.notified = false;
            state.announced = false;
            (was_notified && config.events.monitor_recovered).then_some("monitor_recovered")
        }
        MonitorOutcome::Failed => {
            let start = *state.incident_started_at.get_or_insert(at);
            state.attempts = state.attempts.saturating_add(1);
            if !state.notified
                && state.attempts >= config.monitor_failure.consecutive_attempts
                && at.saturating_sub(start)
                    >= config.monitor_failure.minimum_duration_seconds as i64
            {
                state.notified = true;
                state.announced = config.events.monitor_failed;
                config.events.monitor_failed.then_some("monitor_failed")
            } else {
                None
            }
        }
    };
    let notification = event.map(|kind| Notification {
        event: if kind == "monitor_failed" {
            NotificationEvent::MonitorFailed
        } else {
            NotificationEvent::MonitorRecovered
        },
        observed_at: Some(at),
        provider: provider.into(),
        profile: profile.into(),
        window: "monitor".into(),
        label: "monitor".into(),
        used_percent: 0.0,
        remaining_percent: 0.0,
        resets_at: None,
        threshold: 0.0,
        severity: if kind == "monitor_failed" {
            Severity::High
        } else {
            Severity::Warning
        },
        title: format!("{provider} {kind}"),
        message: format!("{provider} [{profile}]: {kind}"),
    });
    (notification, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(remaining: f64, reset: Option<i64>) -> Quota {
        Quota::new("primary", "5h".into(), Some(300), 100.0 - remaining, reset).unwrap()
    }

    #[test]
    fn crossing_multiple_legacy_thresholds_emits_only_critical_alert() {
        let (notification, _) = evaluate("codex", "default", &quota(4.0, None), None);
        let notification = notification.unwrap();
        assert_eq!(notification.threshold, 95.0);
        assert_eq!(notification.severity, Severity::Critical);
        assert_eq!(notification.used_percent, 96.0);
    }

    #[test]
    fn legacy_levels_deduplicate_and_rearm_as_before() {
        let (first, state) = evaluate("codex", "default", &quota(18.0, None), None);
        assert_eq!(first.unwrap().threshold, 80.0);
        assert!(
            evaluate("codex", "default", &quota(18.0, None), Some(state.clone()))
                .0
                .is_none()
        );
        let (urgent, state) = evaluate("codex", "default", &quota(4.0, None), Some(state));
        assert_eq!(urgent.unwrap().threshold, 95.0);
        let (_, recovered) = evaluate("codex", "default", &quota(50.0, None), Some(state));
        assert_eq!(recovered.level, 100);
    }

    fn configured(
        used: f64,
        reset: Option<i64>,
        old: Option<PolicyState>,
        config: &NotificationPolicyConfig,
        at: i64,
    ) -> (Vec<Notification>, PolicyState) {
        evaluate_configured(
            "codex",
            "default",
            &quota(100.0 - used, reset),
            old,
            config,
            at,
        )
    }

    #[test]
    fn jumps_deduplicate_exhaustion_and_survive_state_serialization() {
        let cfg = NotificationPolicyConfig::default();
        let (first, state) = configured(96.0, None, None, &cfg, 100);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].threshold, 95.0);
        let state: PolicyState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        let (again, state) = configured(97.0, None, Some(state), &cfg, 101);
        assert!(again.is_empty());
        let (exhausted, state) = configured(100.0, None, Some(state), &cfg, 102);
        assert_eq!(exhausted[0].threshold, 100.0);
        let (_, state) = configured(99.0, None, Some(state), &cfg, 103);
        assert!(configured(100.0, None, Some(state), &cfg, 104).0.is_empty());
    }

    #[test]
    fn successive_levels_and_fractional_boundaries() {
        let mut cfg = NotificationPolicyConfig::default();
        let (_, state) = configured(70.0, None, None, &cfg, 100);
        let (events, state) = configured(85.0, None, Some(state), &cfg, 101);
        assert_eq!(events[0].threshold, 80.0);
        let (events, state) = configured(92.0, None, Some(state), &cfg, 102);
        assert_eq!(events[0].threshold, 90.0);
        let (events, state) = configured(96.0, None, Some(state), &cfg, 103);
        assert_eq!(events[0].threshold, 95.0);
        let (events, _) = configured(99.999, None, Some(state), &cfg, 104);
        assert!(events.is_empty());
        cfg.thresholds_used_percent = vec![80.25, 90.5, 95.75, 100.0];
        let (events, state) = configured(80.249, None, None, &cfg, 100);
        assert!(events.is_empty());
        let (events, _) = configured(80.25, None, Some(state), &cfg, 101);
        assert_eq!(events[0].threshold, 80.25);
    }

    #[test]
    fn overrides_and_rule_edits_do_not_replay_old_levels() {
        let mut cfg = NotificationPolicyConfig::default();
        cfg.overrides
            .push(crate::config::NotificationPolicyOverride {
                provider: "codex".into(),
                profile: None,
                window: None,
                thresholds_used_percent: vec![85.0, 95.0],
            });
        cfg.overrides
            .push(crate::config::NotificationPolicyOverride {
                provider: "codex".into(),
                profile: Some("default".into()),
                window: Some("primary".into()),
                thresholds_used_percent: vec![91.5, 98.5],
            });
        assert_eq!(
            effective_thresholds(&cfg, "codex", "default", "primary"),
            &[91.5, 98.5]
        );
        assert_eq!(
            effective_thresholds(&cfg, "codex", "other", "primary"),
            &[85.0, 95.0]
        );
        let (first, state) = configured(92.0, None, None, &cfg, 100);
        assert_eq!(first[0].threshold, 91.5);
        cfg.overrides[1].thresholds_used_percent = vec![80.0, 90.0, 97.0, 100.0];
        let (changed, state) = configured(92.0, None, Some(state), &cfg, 101);
        assert!(changed.is_empty());
        let (next, _) = configured(97.0, None, Some(state), &cfg, 102);
        assert_eq!(next[0].threshold, 97.0);
    }

    #[test]
    fn expiry_or_corrected_anchor_is_not_a_renewal() {
        let cfg = NotificationPolicyConfig::default();
        let (_, state) = configured(96.0, Some(1000), None, &cfg, 100);
        let (early, state) = configured(97.0, Some(2000), Some(state), &cfg, 900);
        assert!(early.is_empty());
        let (expired, state) = configured(97.0, Some(1000), Some(state), &cfg, 1001);
        assert!(expired.is_empty());
        let (corrected, state) = configured(96.0, Some(2000), Some(state), &cfg, 1002);
        assert!(corrected.is_empty());
        assert_eq!(state.generation, 0);
        let (renewed, state) = configured(96.0, Some(3000), Some(state), &cfg, 2001);
        assert_eq!(renewed[0].threshold, 95.0);
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn renewal_and_margin_recovery_have_independent_switches() {
        let mut cfg = NotificationPolicyConfig::default();
        cfg.events.quota_renewed = false;
        let (_, state) = configured(96.0, Some(1000), None, &cfg, 100);
        let (events, _) = configured(70.0, Some(2000), Some(state), &cfg, 1100);
        assert!(events.is_empty());
        cfg.events.quota_renewed = true;
        cfg.events.quota_recovered = false;
        let (_, state) = configured(96.0, Some(1000), None, &cfg, 100);
        let (events, _) = configured(70.0, Some(2000), Some(state), &cfg, 1100);
        assert_eq!(events.len(), 1);
        assert!(events[0].title.contains("renewed"));
        let (_, state) = configured(96.0, None, None, &cfg, 100);
        let (_, state) = configured(74.0, None, Some(state), &cfg, 101);
        let (events, _) = configured(73.0, None, Some(state), &cfg, 102);
        assert!(events.is_empty());
    }

    #[test]
    fn recovery_needs_two_distinct_successful_low_readings() {
        let cfg = NotificationPolicyConfig::default();
        let (_, state) = configured(100.0, None, None, &cfg, 100);
        let (first, state) = configured(75.0, None, Some(state), &cfg, 101);
        assert!(first.is_empty());
        assert!(configured(75.0, None, Some(state.clone()), &cfg, 101)
            .0
            .is_empty());
        let mut interrupted = state;
        interrupted.interrupt_recovery();
        let (after_failure, state) = configured(75.0, None, Some(interrupted), &cfg, 102);
        assert!(after_failure.is_empty());
        let (recovery, state) = configured(75.0, None, Some(state), &cfg, 103);
        assert_eq!(recovery.len(), 1);
        assert!(recovery[0].message.contains("margin recovery observed"));
        assert!(!state.limited);
        assert!(configured(75.0, None, Some(state), &cfg, 104).0.is_empty());
    }

    #[test]
    fn contradictory_reading_cancels_recovery_without_lowering_watermark() {
        let cfg = NotificationPolicyConfig::default();
        let (_, state) = configured(96.0, None, None, &cfg, 100);
        let (_, state) = configured(75.0, None, Some(state), &cfg, 101);
        let (repeat, state) = configured(97.0, None, Some(state), &cfg, 102);
        assert!(repeat.is_empty());
        let (first, state) = configured(75.0, None, Some(state), &cfg, 103);
        assert!(first.is_empty());
        let (confirmed, _) = configured(75.0, None, Some(state), &cfg, 104);
        assert_eq!(confirmed.len(), 1);
    }

    #[test]
    fn legacy_import_does_not_replay_already_covered_level() {
        let cfg = NotificationPolicyConfig::default();
        let state = PolicyState::from_legacy(AlertState {
            reset_at: Some(1000),
            level: 5,
        });
        let (events, state) = configured(96.0, Some(1000), Some(state), &cfg, 100);
        assert!(events.is_empty());
        assert_eq!(state.covered_used_percent, Some(95.0));
    }

    #[test]
    fn monitor_incident_counts_attempts_not_backoff_and_only_recovers_if_announced() {
        let mut cfg = NotificationPolicyConfig::default();
        cfg.events.monitor_failed = true;
        cfg.events.monitor_recovered = true;
        let mut previous = None;
        for at in [100, 400, 699] {
            let (event, state) = evaluate_monitor(
                "codex",
                "default",
                MonitorOutcome::Failed,
                previous,
                &cfg,
                at,
            );
            assert!(event.is_none());
            previous = Some(state);
        }
        let (failed, state) = evaluate_monitor(
            "codex",
            "default",
            MonitorOutcome::Failed,
            previous,
            &cfg,
            700,
        );
        assert!(failed.unwrap().title.contains("monitor_failed"));
        let (repeat, state) = evaluate_monitor(
            "codex",
            "default",
            MonitorOutcome::Failed,
            Some(state),
            &cfg,
            800,
        );
        assert!(repeat.is_none());
        let (recovered, state) = evaluate_monitor(
            "codex",
            "default",
            MonitorOutcome::Succeeded,
            Some(state),
            &cfg,
            801,
        );
        assert!(recovered.unwrap().title.contains("monitor_recovered"));
        assert!(!state.notified);
        let (_, state) = evaluate_monitor(
            "codex",
            "default",
            MonitorOutcome::Failed,
            Some(state),
            &cfg,
            1000,
        );
        let (auth, state) = evaluate_monitor(
            "codex",
            "default",
            MonitorOutcome::AuthChanged,
            Some(state),
            &cfg,
            1001,
        );
        assert!(auth.is_none());
        assert_eq!(state.attempts, 0);
    }
}
