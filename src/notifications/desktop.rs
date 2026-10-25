use super::{DeliveryError, Notification, Severity, Transport};
use std::{
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub struct Desktop;

impl Transport for Desktop {
    fn name(&self) -> &'static str {
        "desktop"
    }

    fn send(&mut self, notification: &Notification) -> Result<(), DeliveryError> {
        let mut message = format!(
            "{}\nProvider: {}\nProfile: {}\nWindow: {}\nEvent: {:?}",
            notification.message,
            notification.provider,
            notification.profile,
            notification.window,
            notification.event
        );
        if notification.window != "monitor" {
            message.push_str(&format!(
                "\nConsumed: {:?}%\nAvailable: {:?}%",
                notification.used_percent, notification.remaining_percent
            ));
            if let Some(reset) = notification.resets_at {
                message.push_str(&format!("\nResets at (UTC): {reset}"));
            }
        }
        if let Some(at) = notification.observed_at {
            message.push_str(&format!("\nObserved (UTC): {at}"));
        }
        let mut child = Command::new("notify-send")
            .process_group(0)
            .args([
                "-a",
                "AI Quota Monitor",
                "-u",
                if matches!(
                    notification.severity,
                    Severity::Critical | Severity::Exhausted
                ) {
                    "critical"
                } else {
                    "normal"
                },
                &notification.title,
                &message,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| DeliveryError::Permanent)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(DeliveryError::Permanent)
                    }
                }
                _ if Instant::now() >= deadline => {
                    // SAFETY: this process group was created exclusively for this notification.
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DeliveryError::Transient { retry_after: None });
                }
                _ => thread::sleep(Duration::from_millis(25)),
            }
        }
    }
}
