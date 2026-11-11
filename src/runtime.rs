//! Shared read-only runtime view used by the CLI and TUI.
//!
//! This module never writes fan state and never talks to NBFC directly. The
//! supervisor is the only process that may connect to the NBFC socket; the CLI
//! and TUI obtain backend state from it over IPC. This is essential when the
//! NBFC socket is restricted to `0600 root:root`: an ordinary CLI user cannot
//! connect, and must not mistake that expected denial for a backend outage.

use crate::model::SensorSnapshot;
use crate::nbfc::{NbfcCapabilities, NbfcSensorConfig, NbfcStatus, NbfcVersion};
use crate::policy::fan_role;
use crate::regulation::{FanObservation, mapping_matches};
use crate::sensors::{SysfsRoot, collect_all};
use crate::service::{ServiceStatus, nbfc_service_status};
use crate::supervisor::{SupervisorProbe, probe as probe_supervisor};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct HostInfo {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub bios: Option<String>,
    pub kernel: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeView {
    pub snapshot: SensorSnapshot,
    pub version: Option<NbfcVersion>,
    pub capabilities: Option<NbfcCapabilities>,
    pub status: Option<NbfcStatus>,
    pub service: ServiceStatus,
    pub sensor_config: Option<NbfcSensorConfig>,
    pub backend_error: Option<String>,
    pub supervisor: SupervisorProbe,
}

/// Decode one optional field of the supervisor status document.
///
/// Fields are decoded independently so a document produced by a different
/// NeoSense build degrades field by field instead of being discarded whole.
fn field<T: serde::de::DeserializeOwned>(value: &serde_json::Value, key: &str) -> Option<T> {
    value
        .get(key)
        .and_then(|field| serde_json::from_value(field.clone()).ok())
}

/// Build the read-only view used by the CLI and TUI.
///
/// Backend state comes from the supervisor over IPC. Local sensor collection
/// remains local because it is unprivileged, read-only sysfs access. When the
/// supervisor is unavailable, backend fields are reported as unavailable and
/// no NBFC connection is attempted from this process.
pub fn read_view(socket: &Path) -> RuntimeView {
    let supervisor = probe_supervisor(socket);
    let snapshot = collect_all(&SysfsRoot::default());

    if let Some(value) = &supervisor.status {
        let nbfc_version: Option<String> = field(value, "nbfc_version");
        return RuntimeView {
            snapshot,
            version: nbfc_version.as_deref().and_then(NbfcVersion::parse),
            capabilities: field(value, "capabilities"),
            status: field(value, "nbfc"),
            service: field(value, "service").unwrap_or_else(nbfc_service_status),
            sensor_config: field(value, "sensor_config"),
            backend_error: field(value, "last_error"),
            supervisor,
        };
    }

    let backend_error = Some(format!(
        "supervisor {}: {}",
        supervisor.state.label(),
        supervisor
            .error
            .as_deref()
            .unwrap_or("backend state is unavailable without the supervisor")
    ));
    RuntimeView {
        snapshot,
        version: None,
        capabilities: None,
        status: None,
        service: nbfc_service_status(),
        sensor_config: None,
        backend_error,
        supervisor,
    }
}

pub fn fan_observations(view: &RuntimeView) -> Vec<FanObservation> {
    let Some(status) = &view.status else {
        return Vec::new();
    };
    status
        .fans
        .iter()
        .map(|fan| {
            let role = fan_role(&fan.name);
            let source = view
                .sensor_config
                .as_ref()
                .and_then(|config| config.fans.iter().find(|f| f.index == fan.index));
            let mapping_ok = source
                .map(|source| mapping_matches(role, &source.sensors))
                .unwrap_or(false);
            FanObservation {
                fan_index: fan.index,
                fan_name: fan.name.clone(),
                role,
                mapping_ok,
                // Local previews have no previous policy target. The current
                // NBFC target may be an automatic-curve value and must not
                // distort the reference curve.
                previous_target_percent: None,
                previous_temperature_c: None,
            }
        })
        .collect()
}

pub fn host_info() -> HostInfo {
    HostInfo {
        vendor: read_dmi("sys_vendor"),
        model: read_dmi("product_name"),
        bios: read_dmi("bios_version"),
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string()),
    }
}

fn read_dmi(name: &str) -> Option<String> {
    std::fs::read_to_string(Path::new("/sys/class/dmi/id").join(name))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{Response, read_message, write_message};
    use std::io::BufReader;
    use std::os::unix::net::UnixListener;

    /// Serve exactly one `status.get` request and return the socket path's
    /// parent temp dir (kept alive by the caller).
    fn supervisor_with_status(
        status: serde_json::Value,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("supervisor.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let request: crate::ipc::Request = read_message(&mut reader).unwrap();
            write_message(&mut stream, &Response::ok(request.id, status)).unwrap();
        });
        (dir, socket)
    }

    #[test]
    fn backend_state_comes_from_the_supervisor_not_the_nbfc_cli() {
        let (_dir, socket) = supervisor_with_status(serde_json::json!({
            "nbfc_version": "9.9.9",
            "capabilities": {
                "version": {"major": 9, "minor": 9, "patch": 9},
                "json_status": true,
                "reset_ec": true,
                "config_apply": false,
                "protected_manual": true,
                "access_controlled": true,
                "socket": {
                    "path": "/run/nbfc_service.socket",
                    "exists": true,
                    "is_socket": true,
                    "mode": "600",
                    "uid": 0,
                    "gid": 0,
                    "group_name": "root",
                    "ordinary_group_members": 0,
                    "group_or_world_writable": false,
                    "acl_present": false,
                    "supervisor_uid": 0,
                    "supervisor_gid": 0,
                    "supervisor_groups_verified": true,
                    "identity_source": "current process",
                    "accessible_to_supervisor": true,
                    "access_controlled": true,
                    "error": null
                }
            },
            "nbfc": {
                "version": null,
                "read_only": false,
                "selected_config": "Acer Nitro AN515-57 Custom",
                "fans": [],
                "warnings": []
            },
            "service": {"active_state": "active", "main_pid": 4242},
            "sensor_config": {"fans": []},
            "last_error": null
        }));

        let view = read_view(&socket);
        // The host `nbfc --version` is not 9.9.9; seeing it proves the value
        // came from IPC and that no local NBFC process was consulted.
        assert_eq!(
            view.version.map(|v| v.to_string()).as_deref(),
            Some("9.9.9")
        );
        assert_eq!(view.service.main_pid, Some(4242));
        assert!(view.capabilities.unwrap().protected_manual);
        assert_eq!(
            view.status
                .as_ref()
                .and_then(|s| s.selected_config.as_deref()),
            Some("Acer Nitro AN515-57 Custom")
        );
        assert!(view.backend_error.is_none());
        assert!(view.supervisor.state.is_connected());
    }

    #[test]
    fn missing_supervisor_reports_supervisor_unavailable_without_nbfc_access() {
        let dir = tempfile::tempdir().unwrap();
        let view = read_view(&dir.path().join("missing.sock"));
        assert!(view.version.is_none());
        assert!(view.status.is_none());
        assert!(!view.supervisor.state.is_connected());
        let error = view.backend_error.unwrap();
        assert!(error.contains("supervisor"), "{error}");
        assert!(!error.contains("connect()"), "{error}");
    }

    #[test]
    fn supervisor_last_error_is_preserved() {
        let (_dir, socket) = supervisor_with_status(serde_json::json!({
            "nbfc_version": "0.5.4",
            "nbfc": null,
            "last_error": "NBFC status unavailable"
        }));
        let view = read_view(&socket);
        assert_eq!(
            view.backend_error.as_deref(),
            Some("NBFC status unavailable")
        );
        assert!(view.status.is_none());
    }
}
