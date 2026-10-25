use super::{
    ntfy::{Ntfy, NtfyConfig},
    Notification, Transport,
};
use crate::config::Config;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MESSAGE_TTL: Duration = Duration::from_secs(600);
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(30)];
const QUEUE_CAPACITY: usize = 32;

fn retry_deadline(
    now: Instant,
    message_deadline: Instant,
    requested: Option<Duration>,
    attempt: usize,
) -> Option<Instant> {
    let delay = requested
        .unwrap_or(RETRY_DELAYS[attempt])
        .max(RETRY_DELAYS[attempt]);
    if delay >= message_deadline.saturating_duration_since(now) {
        return None;
    }
    Some(now + delay)
}

struct QueuedNotification {
    notification: Notification,
    created_at: Instant,
    deadline: Instant,
}

struct Worker {
    sender: SyncSender<QueuedNotification>,
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

pub struct Runtime {
    workers: Vec<Worker>,
    pending: Arc<std::sync::atomic::AtomicUsize>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestDeliveryStatus {
    Accepted,
    Failed,
    TimedOut,
    NotEnabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDeliveryResult {
    pub transport: &'static str,
    pub status: TestDeliveryStatus,
}

impl Runtime {
    /// Sends a diagnostic event through the configured transports and waits for
    /// each result up to one shared deadline. It does not start monitoring workers.
    pub fn test_transports(
        config: &Config,
        notification: Notification,
        timeout: Duration,
    ) -> Vec<TestDeliveryResult> {
        let desktop_enabled = config.notifications && config.notification_transports.desktop;
        let ntfy_enabled = config.notifications
            && config
                .notification_transports
                .ntfy
                .as_ref()
                .is_some_and(|n| n.enabled);
        let outcomes = vec![
            TestDeliveryResult {
                transport: "desktop",
                status: TestDeliveryStatus::NotEnabled,
            },
            TestDeliveryResult {
                transport: "ntfy",
                status: TestDeliveryStatus::NotEnabled,
            },
        ];
        let mut active: Vec<Box<dyn Transport>> = Vec::new();
        if desktop_enabled {
            active.push(Box::new(super::desktop::Desktop));
        }
        if ntfy_enabled {
            if let Some(ntfy) = config
                .notification_transports
                .ntfy
                .as_ref()
                .filter(|n| n.enabled)
            {
                active.push(Box::new(Ntfy::new(NtfyConfig {
                    server_url: ntfy.server_url.trim_end_matches('/').to_owned(),
                    topic: ntfy.topic.clone(),
                    token_file: ntfy.token_file.clone(),
                })));
            }
        }
        Self::deliver_test(active, notification, timeout, outcomes)
    }

    fn deliver_test(
        active: Vec<Box<dyn Transport>>,
        notification: Notification,
        timeout: Duration,
        mut outcomes: Vec<TestDeliveryResult>,
    ) -> Vec<TestDeliveryResult> {
        let (tx, rx) = mpsc::channel();
        let mut waiting = 0;
        for mut transport in active {
            let name = transport.name();
            let sender = tx.clone();
            let message = notification.clone();
            match thread::Builder::new()
                .name(format!("quota-notification-test-{name}"))
                .spawn(move || {
                    let accepted = transport.send(&message).is_ok();
                    let _ = sender.send((name, accepted));
                }) {
                Ok(_) => {
                    waiting += 1;
                    if let Some(result) =
                        outcomes.iter_mut().find(|result| result.transport == name)
                    {
                        result.status = TestDeliveryStatus::TimedOut;
                    }
                }
                Err(_) => {
                    if let Some(result) =
                        outcomes.iter_mut().find(|result| result.transport == name)
                    {
                        result.status = TestDeliveryStatus::Failed;
                    }
                }
            }
        }
        drop(tx);
        let deadline = Instant::now() + timeout;
        while waiting > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok((name, accepted)) => {
                    waiting -= 1;
                    if let Some(result) =
                        outcomes.iter_mut().find(|result| result.transport == name)
                    {
                        result.status = if accepted {
                            TestDeliveryStatus::Accepted
                        } else {
                            TestDeliveryStatus::Failed
                        };
                    }
                }
                Err(_) => break,
            }
        }
        outcomes
    }

    pub fn from_config(config: &Config) -> Self {
        Self::from_config_with_spawn(config, true)
    }

    #[cfg(test)]
    pub(crate) fn from_config_simulating_spawn_failure(config: &Config) -> Self {
        Self::from_config_with_spawn(config, false)
    }

    fn from_config_with_spawn(config: &Config, allow_spawn: bool) -> Self {
        let mut transports: Vec<Box<dyn Transport>> = Vec::new();
        if config.notifications && config.notification_transports.desktop {
            transports.push(Box::new(super::desktop::Desktop));
        }
        if config.notifications {
            if let Some(ntfy) = config
                .notification_transports
                .ntfy
                .as_ref()
                .filter(|n| n.enabled)
            {
                transports.push(Box::new(Ntfy::new(NtfyConfig {
                    server_url: ntfy.server_url.trim_end_matches('/').to_owned(),
                    topic: ntfy.topic.clone(),
                    token_file: ntfy.token_file.clone(),
                })));
            }
        }
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        Self::with_transports(transports, pending, allow_spawn)
    }

    fn with_transports(
        transports: Vec<Box<dyn Transport>>,
        pending: Arc<std::sync::atomic::AtomicUsize>,
        allow_spawn: bool,
    ) -> Self {
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut workers = Vec::new();
        for transport in transports {
            match Self::worker(
                transport,
                Arc::clone(&pending),
                Arc::clone(&dropped),
                allow_spawn,
            ) {
                Ok(worker) => workers.push(worker),
                Err(name) => {
                    eprintln!("ERROR event=notification_worker_start_failed transport={name}")
                }
            }
        }
        Self {
            workers,
            pending,
            dropped,
        }
    }

    fn worker(
        mut transport: Box<dyn Transport>,
        pending: Arc<std::sync::atomic::AtomicUsize>,
        dropped: Arc<std::sync::atomic::AtomicUsize>,
        allow_spawn: bool,
    ) -> Result<Worker, &'static str> {
        let (sender, receiver) = mpsc::sync_channel::<QueuedNotification>(QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let name = transport.name();
        if !allow_spawn {
            return Err(name);
        }
        let join = thread::Builder::new()
            .name(format!("quota-notify-{name}"))
            .spawn(move || {
                let mut backlog = VecDeque::new();
                while !stopped.load(Ordering::Acquire) {
                    let queued = match backlog.pop_front() {
                        Some(queued) => queued,
                        None => match receiver.recv_timeout(Duration::from_millis(100)) {
                            Ok(queued) => queued,
                            Err(_) => continue,
                        },
                    };
                    let mut attempt = 0;
                    loop {
                        if stopped.load(Ordering::Acquire) {
                            break;
                        }
                        if queued.created_at.elapsed() >= MESSAGE_TTL
                            || Instant::now() >= queued.deadline
                        {
                            break;
                        }
                        match transport.send(&queued.notification) {
                            Ok(()) => break,
                            Err(super::DeliveryError::Permanent) => {
                                eprintln!(
                                    "WARN event=notification_delivery_failed transport={name}"
                                );
                                break;
                            }
                            Err(super::DeliveryError::Transient { retry_after }) if attempt < 2 => {
                                let Some(until) = retry_deadline(
                                    Instant::now(),
                                    queued.deadline,
                                    retry_after,
                                    attempt,
                                ) else {
                                    break;
                                };
                                while !stopped.load(Ordering::Acquire)
                                    && Instant::now() < until
                                    && Instant::now() < queued.deadline
                                {
                                    // Keep the receiver responsive and retain only bounded queued work.
                                    match receiver.recv_timeout(Duration::from_millis(100)) {
                                        Ok(next) if backlog.len() < QUEUE_CAPACITY => {
                                            backlog.push_back(next)
                                        }
                                        Ok(_) => {
                                            pending.fetch_sub(1, Ordering::AcqRel);
                                            dropped.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(_) => (),
                                    }
                                }
                                attempt += 1;
                            }
                            Err(super::DeliveryError::Transient { .. }) => eprintln!(
                                "WARN event=notification_delivery_failed transport={name}"
                            ),
                        }
                    }
                    pending.fetch_sub(1, Ordering::AcqRel);
                }
            })
            .map_err(|_| name)?;
        Ok(Worker { sender, stop, join })
    }

    pub fn submit(&self, notification: Notification) {
        for worker in &self.workers {
            self.pending.fetch_add(1, Ordering::AcqRel);
            let now = Instant::now();
            if worker
                .sender
                .try_send(QueuedNotification {
                    notification: notification.clone(),
                    created_at: now,
                    deadline: now + MESSAGE_TTL,
                })
                .is_err()
            {
                self.pending.fetch_sub(1, Ordering::AcqRel);
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// A bounded foreground flush for one-shot `refresh`; daemon delivery remains asynchronous.
    pub fn flush_for(&self, duration: Duration) {
        let deadline = std::time::Instant::now() + duration;
        while self.pending.load(Ordering::Acquire) > 0 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        self.shutdown();
    }

    pub fn shutdown(&self) {
        for worker in &self.workers {
            worker.stop.store(true, Ordering::Release);
        }
    }

    #[cfg(test)]
    pub(crate) fn workers_empty_for_test(&self) -> bool {
        self.workers.is_empty()
    }

    fn report_dropped(&self) {
        let count = self.dropped.swap(0, Ordering::AcqRel);
        if count > 0 {
            eprintln!("WARN event=notification_queue_full dropped={count}");
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        for worker in &self.workers {
            worker.stop.store(true, Ordering::Release);
        }
        // Join handles are consumed here; transport deadlines keep this bounded.
        for worker in self.workers.drain(..) {
            let _ = worker.join.join();
        }
        self.report_dropped();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Fake;
    impl Transport for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn send(&mut self, _: &Notification) -> Result<(), super::super::DeliveryError> {
            Ok(())
        }
    }

    struct DiagnosticFake {
        label: &'static str,
        result: Result<(), super::super::DeliveryError>,
        pause: Duration,
    }
    impl Transport for DiagnosticFake {
        fn name(&self) -> &'static str {
            self.label
        }
        fn send(&mut self, _: &Notification) -> Result<(), super::super::DeliveryError> {
            thread::sleep(self.pause);
            self.result.clone()
        }
    }

    #[test]
    fn failed_worker_creation_disables_only_that_transport_without_panicking() {
        let runtime =
            Runtime::with_transports(vec![Box::new(Fake)], Arc::new(AtomicUsize::new(0)), false);
        assert!(runtime.workers.is_empty());
    }

    #[test]
    fn queued_notification_has_fixed_monotonic_ttl() {
        let created = Instant::now();
        let queued = QueuedNotification {
            notification: test_notification(),
            created_at: created,
            deadline: created + MESSAGE_TTL,
        };
        assert_eq!(
            queued.deadline.duration_since(queued.created_at),
            Duration::from_secs(600)
        );
        assert!(Instant::now() < queued.deadline);
    }

    #[test]
    fn retry_beyond_remaining_ttl_is_discarded() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(10);
        assert_eq!(
            retry_deadline(now, deadline, Some(Duration::from_secs(12)), 0),
            None
        );
        assert!(retry_deadline(now, deadline, Some(Duration::from_secs(2)), 0).is_some());
    }

    #[test]
    fn diagnostic_waits_for_each_fake_transport_result_and_reports_disabled() {
        let active: Vec<Box<dyn Transport>> = vec![
            Box::new(DiagnosticFake {
                label: "desktop",
                result: Ok(()),
                pause: Duration::ZERO,
            }),
            Box::new(DiagnosticFake {
                label: "ntfy",
                result: Err(super::super::DeliveryError::Permanent),
                pause: Duration::ZERO,
            }),
        ];
        let results = Runtime::deliver_test(
            active,
            test_notification(),
            Duration::from_secs(1),
            vec![
                TestDeliveryResult {
                    transport: "desktop",
                    status: TestDeliveryStatus::TimedOut,
                },
                TestDeliveryResult {
                    transport: "ntfy",
                    status: TestDeliveryStatus::TimedOut,
                },
                TestDeliveryResult {
                    transport: "unused",
                    status: TestDeliveryStatus::NotEnabled,
                },
            ],
        );
        assert_eq!(results[0].status, TestDeliveryStatus::Accepted);
        assert_eq!(results[1].status, TestDeliveryStatus::Failed);
        assert_eq!(results[2].status, TestDeliveryStatus::NotEnabled);
    }

    #[test]
    fn diagnostic_timeout_is_not_reported_as_success() {
        let results = Runtime::deliver_test(
            vec![Box::new(DiagnosticFake {
                label: "desktop",
                result: Ok(()),
                pause: Duration::from_millis(40),
            })],
            test_notification(),
            Duration::from_millis(1),
            vec![TestDeliveryResult {
                transport: "desktop",
                status: TestDeliveryStatus::TimedOut,
            }],
        );
        assert_eq!(results[0].status, TestDeliveryStatus::TimedOut);
    }

    fn test_notification() -> Notification {
        Notification {
            event: crate::notifications::NotificationEvent::Threshold,
            observed_at: Some(100),
            provider: "codex".into(),
            profile: "default".into(),
            window: "5h".into(),
            label: "primary".into(),
            used_percent: 96.0,
            remaining_percent: 4.0,
            resets_at: None,
            threshold: 95.0,
            severity: super::super::Severity::Critical,
            title: "test".into(),
            message: "test".into(),
        }
    }
}
