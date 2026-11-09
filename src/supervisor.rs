//! Shared supervisor/IPC probing and classification.
//!
//! The same states are consumed by the CLI, TUI and diagnostics. A missing
//! socket, a permission failure, a stale socket and a protocol failure are
//! deliberately distinct.

use crate::ipc::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorState {
    Healthy,
    Degraded,
    Connected,
    NotRunning,
    PermissionDenied,
    StaleSocket,
    ProtocolError,
}

impl SupervisorState {
    pub fn label(self) -> &'static str {
        match self {
            SupervisorState::Healthy => "healthy",
            SupervisorState::Degraded => "degraded",
            SupervisorState::Connected => "connected",
            SupervisorState::NotRunning => "not running",
            SupervisorState::PermissionDenied => "permission denied",
            SupervisorState::StaleSocket => "stale socket",
            SupervisorState::ProtocolError => "protocol error",
        }
    }

    pub fn is_connected(self) -> bool {
        matches!(
            self,
            SupervisorState::Healthy | SupervisorState::Degraded | SupervisorState::Connected
        )
    }

    pub fn severity(self) -> &'static str {
        match self {
            SupervisorState::Healthy => "ok",
            SupervisorState::Connected => "info",
            SupervisorState::Degraded => "warning",
            SupervisorState::NotRunning
            | SupervisorState::PermissionDenied
            | SupervisorState::StaleSocket
            | SupervisorState::ProtocolError => "critical",
        }
    }

    pub fn action(self) -> &'static str {
        match self {
            SupervisorState::Healthy | SupervisorState::Connected => "No action required.",
            SupervisorState::Degraded => {
                "Inspect the System view or `neosense diagnose`; automatic recovery may be in progress."
            }
            SupervisorState::NotRunning => {
                "Start `neosensed`; verify it is enabled and that its unit is installed."
            }
            SupervisorState::PermissionDenied => {
                "Log out and back in, or run `newgrp neosense`; then verify socket ownership/mode."
            }
            SupervisorState::StaleSocket => {
                "Remove the stale runtime socket only after confirming no supervisor is running, then restart `neosensed`."
            }
            SupervisorState::ProtocolError => {
                "Restart `neosensed` and inspect its journal; the IPC protocol did not respond correctly."
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorProbe {
    pub state: SupervisorState,
    pub socket: PathBuf,
    pub socket_exists: bool,
    pub socket_mode: Option<String>,
    pub socket_uid: Option<u32>,
    pub socket_gid: Option<u32>,
    pub health: Option<bool>,
    pub reason: Option<String>,
    pub error: Option<String>,
    pub status: Option<Value>,
}

impl SupervisorProbe {
    fn base(state: SupervisorState, socket: &Path) -> Self {
        let metadata = std::fs::metadata(socket).ok();
        Self {
            state,
            socket: socket.to_path_buf(),
            socket_exists: metadata.is_some(),
            socket_mode: metadata.as_ref().map(|m| format!("{:o}", m.mode() & 0o777)),
            socket_uid: metadata.as_ref().map(MetadataExt::uid),
            socket_gid: metadata.as_ref().map(MetadataExt::gid),
            health: None,
            reason: None,
            error: None,
            status: None,
        }
    }

    fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn detail(&self) -> String {
        match self.state {
            SupervisorState::Healthy | SupervisorState::Connected => {
                format!("{} — socket {}", self.state.label(), self.socket.display())
            }
            _ => format!(
                "{}: {}",
                self.state.label(),
                self.error.as_deref().unwrap_or("no additional detail")
            ),
        }
    }
}

pub fn probe(socket: &Path) -> SupervisorProbe {
    let metadata = match std::fs::metadata(socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SupervisorProbe::base(SupervisorState::NotRunning, socket)
                .with_error("socket was not created; the supervisor is not running");
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return SupervisorProbe::base(SupervisorState::PermissionDenied, socket)
                .with_error("runtime directory/socket is not accessible to the current user");
        }
        Err(error) => {
            return SupervisorProbe::base(SupervisorState::ProtocolError, socket)
                .with_error(format!("cannot inspect socket: {error}"));
        }
    };

    let mut probe = SupervisorProbe::base(SupervisorState::NotRunning, socket);
    probe.socket_exists = true;
    probe.socket_mode = Some(format!("{:o}", metadata.mode() & 0o777));
    probe.socket_uid = Some(metadata.uid());
    probe.socket_gid = Some(metadata.gid());

    match Client::connect(socket) {
        Ok(mut client) => match client.call("status.get", serde_json::json!({})) {
            Ok(value) => {
                probe.status = Some(value.clone());
                probe.health = value
                    .get("health")
                    .and_then(|health| health.get("healthy"))
                    .and_then(Value::as_bool);
                probe.reason = value
                    .get("health")
                    .and_then(|health| health.get("reason"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                probe.state = match probe.health {
                    Some(true) => SupervisorState::Healthy,
                    Some(false) => SupervisorState::Degraded,
                    None => SupervisorState::Connected,
                };
                probe
            }
            Err(error) => {
                probe.state = SupervisorState::ProtocolError;
                probe.with_error(error.message)
            }
        },
        Err(error) => {
            probe.state = if error.code == "permission_denied" {
                SupervisorState::PermissionDenied
            } else if error.code == "ipc_connect" {
                SupervisorState::StaleSocket
            } else {
                SupervisorState::ProtocolError
            };
            probe.with_error(error.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{Response, read_message, write_message};
    use std::io::BufReader;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    #[test]
    fn missing_socket_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let probe = probe(&dir.path().join("missing.sock"));
        assert_eq!(probe.state, SupervisorState::NotRunning);
        assert!(probe.error.unwrap().contains("not running"));
    }

    #[test]
    fn permission_denied_is_distinct() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("restricted.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o000)).unwrap();
        let probe = probe(&socket);
        assert_eq!(probe.state, SupervisorState::PermissionDenied);
        let error = probe.error.unwrap().to_ascii_lowercase();
        assert!(error.contains("denied") || error.contains("not accessible"));
    }

    #[test]
    fn healthy_protocol_is_classified() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("healthy.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let request: crate::ipc::Request = read_message(&mut reader).unwrap();
            write_message(
                &mut stream,
                &Response::ok(
                    request.id,
                    serde_json::json!({"health":{"healthy":true},"sensors":{}}),
                ),
            )
            .unwrap();
        });
        let probe = probe(&socket);
        assert_eq!(probe.state, SupervisorState::Healthy);
        assert_eq!(probe.health, Some(true));
        handle.join().unwrap();
    }
}
