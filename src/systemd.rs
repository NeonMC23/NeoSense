//! Small systemd notification adapter.
//!
//! The supervisor sends `READY=1` only after the IPC socket and initial state
//! are established, sends `WATCHDOG=1` only while `Daemon::health()` says the
//! core is healthy, and sends `STOPPING=1` during orderly shutdown.

use sd_notify::{NotifyState, notify, watchdog_enabled};
use std::time::Duration;

pub trait Notifier: Send + Sync {
    fn ready(&self) -> Result<(), String>;
    fn watchdog(&self) -> Result<(), String>;
    fn stopping(&self) -> Result<(), String>;
    fn watchdog_interval(&self) -> Option<Duration>;
}

#[derive(Debug, Default)]
pub struct SystemdNotifier {
    watchdog: Option<Duration>,
}

impl SystemdNotifier {
    pub fn from_env() -> Self {
        Self {
            watchdog: watchdog_enabled(),
        }
    }
}

impl Notifier for SystemdNotifier {
    fn ready(&self) -> Result<(), String> {
        notify(&[
            NotifyState::Ready,
            NotifyState::Status("NeoSense supervisor ready"),
        ])
        .map_err(|e| format!("READY=1: {e}"))
    }

    fn watchdog(&self) -> Result<(), String> {
        notify(&[NotifyState::Watchdog]).map_err(|e| format!("WATCHDOG=1: {e}"))
    }

    fn stopping(&self) -> Result<(), String> {
        notify(&[
            NotifyState::Stopping,
            NotifyState::Status("NeoSense supervisor stopping"),
        ])
        .map_err(|e| format!("STOPPING=1: {e}"))
    }

    fn watchdog_interval(&self) -> Option<Duration> {
        self.watchdog
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_notify_socket_is_not_an_error() {
        // In the test environment NOTIFY_SOCKET is normally absent, so systemd
        // notifications are a no-op rather than a startup failure.
        let notifier = SystemdNotifier::from_env();
        assert!(notifier.ready().is_ok());
        assert!(notifier.watchdog().is_ok());
        assert!(notifier.stopping().is_ok());
    }
}
