//! Shared warning model for the CLI and TUI.
//!
//! Severity is textual as well as coloured: a warning must remain meaningful
//! without colour and must explain the cause, the safety consequence and the
//! next safe action.

use crate::model::SensorStatus;
use crate::nbfc::NbfcSocketSecurity;
use crate::nbfc_config::HardwareIdentityReport;
use crate::policy::fan_role;
use crate::runtime::RuntimeView;
use crate::supervisor::{SupervisorProbe, SupervisorState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
    Success,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Warning => "WARNING",
            Severity::Critical => "CRITICAL",
            Severity::Success => "OK",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub action: String,
}

impl Warning {
    pub fn new(
        code: impl Into<String>,
        severity: Severity,
        title: impl Into<String>,
        detail: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            title: title.into(),
            detail: detail.into(),
            action: action.into(),
        }
    }
}

/// A security warning, deliberately separate from hardware capability
/// warnings, when the NBFC socket can be written by an untrusted local client.
pub fn from_socket_security(security: &NbfcSocketSecurity) -> Option<Warning> {
    if !security.exists || security.access_controlled {
        return None;
    }
    let mode = security.mode.as_deref().unwrap_or("unknown");
    let detail = if !security.is_socket {
        format!(
            "{} exists but is not a Unix socket (mode {mode}); the backend boundary is not verifiable.",
            security.path
        )
    } else if security.group_or_world_writable {
        format!(
            "{} is mode {mode} (uid {:?}, gid {:?}); group/world write permission lets another local client bypass the NeoSense supervisor. {}",
            security.path,
            security.uid,
            security.gid,
            security.error.clone().unwrap_or_default()
        )
    } else {
        format!(
            "{} is mode {mode} (uid {:?}, gid {:?}); {}. Supervisor identity: {}.",
            security.path,
            security.uid,
            security.gid,
            security
                .error
                .clone()
                .unwrap_or_else(|| "access control is not established".to_string()),
            security.identity_source
        )
    };
    let title = if security.group_or_world_writable {
        "NBFC socket permits local bypass"
    } else {
        "NBFC socket access control is not established"
    };
    Some(Warning::new(
        "NBFC_SOCKET_INSECURE",
        Severity::Warning,
        title,
        detail,
        "Do not describe NeoSense as an exclusive control boundary. Require an upstream NBFC socket-permission fix or a verified restrictive service configuration; do not chmod in a loop.",
    ))
}

/// Warnings derived from read-only NBFC configuration identity evidence.
pub fn from_identity(report: &HardwareIdentityReport) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let invalid: Vec<&str> = report
        .fans
        .iter()
        .filter(|fan| !fan.readback_configured)
        .map(|fan| fan.name.as_str())
        .collect();
    if !invalid.is_empty() {
        warnings.push(Warning::new(
            "NBFC_READBACK_CONFIG_INVALID",
            Severity::Warning,
            "NBFC readback range is not configured",
            format!(
                "{} has MinSpeedValueRead/MaxSpeedValueRead missing or invalid; EC CurrentSpeed cannot be interpreted as a speed percentage.",
                invalid.join(", ")
            ),
            "Do not treat EC speed as measured evidence; run `neosense hardware identity` and `neosense diagnose`.",
        ));
    }
    if report.fans.len() > 1
        && report.warnings.iter().any(|warning| {
            warning.contains("register pairs") || warning.contains("physical fan identity")
        })
    {
        warnings.push(Warning::new(
            "NBFC_FAN_IDENTITY_LOGICAL_ONLY",
            Severity::Info,
            "Fan identity evidence is logical only",
            "The selected configuration exposes separate register pairs, but physical fan identity and independent airflow are not verified.",
            "Keep automatic control; manual per-fan control requires independent physical feedback and explicit authorization.",
        ));
    }
    warnings
}

pub fn from_view(view: &RuntimeView, supervisor: Option<&SupervisorProbe>) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let now = crate::model::now_ms();
    let ttl = 5_000;

    let supervisor_connected = supervisor
        .map(|probe| probe.state.is_connected())
        .unwrap_or_else(|| view.supervisor.state.is_connected());
    if view.backend_error.is_some() || view.status.is_none() {
        if supervisor_connected {
            warnings.push(Warning::new(
                "NBFC_UNAVAILABLE",
                Severity::Critical,
                "NBFC backend unavailable",
                "The supervisor is connected but cannot confirm fan state and automatic control.",
                "Inspect `journalctl -u neosensed` and `nbfc_service`; do not attempt manual fan control.",
            ));
        } else {
            warnings.push(Warning::new(
                "SUPERVISOR_UNAVAILABLE",
                Severity::Critical,
                "NeoSense supervisor unavailable",
                "Fan state and automatic control cannot be verified without the supervisor.",
                "Start `neosensed` and verify its socket; the CLI and TUI never access NBFC directly.",
            ));
        }
    }
    // Prefer the supervisor's inspection: only it holds the credentials that
    // matter for a root-only NBFC socket.
    let socket_security = view
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.socket.clone())
        .unwrap_or_else(crate::nbfc::inspect_default_socket);
    if let Some(warning) = from_socket_security(&socket_security) {
        warnings.push(warning);
    }
    if !view.service.is_active() {
        warnings.push(Warning::new(
            "NBFC_SERVICE_INACTIVE",
            Severity::Warning,
            "NBFC service is not active",
            "Automatic fan control may not be updating.",
            "Start or inspect `nbfc_service` before changing fan policy.",
        ));
    }

    if view.snapshot.cpu_package(now, ttl).is_none() {
        warnings.push(Warning::new(
            "CPU_SENSOR_UNAVAILABLE",
            Severity::Critical,
            "No trustworthy CPU temperature",
            "Manual control is blocked; missing data is never treated as a safe temperature.",
            "Run `neosense diagnose --json` and check hwmon/coretemp/k10temp.",
        ));
    }
    if view.snapshot.gpu_indexes().is_empty() {
        warnings.push(Warning::new(
            "GPU_SENSOR_UNAVAILABLE",
            Severity::Warning,
            "No GPU temperature sensor detected",
            "GPU fan control cannot be justified from GPU telemetry.",
            "Keep automatic control; check the NVIDIA/AMD driver if a discrete GPU is present.",
        ));
    }

    if let Some(status) = &view.status {
        if status.fans.is_empty() {
            warnings.push(Warning::new(
                "NO_FANS",
                Severity::Critical,
                "No NBFC fan detected",
                "NeoSense cannot identify a controllable fan.",
                "Verify the selected NBFC configuration and `nbfc status`.",
            ));
        }
        for fan in &status.fans {
            let current = fan.current_speed;
            let target = fan.target_speed;
            if let Some(cur) = current
                && !(0.0..=100.0).contains(&cur)
            {
                warnings.push(Warning::new(
                    format!("FAN_{}_READBACK_RANGE", fan.index),
                    Severity::Critical,
                    format!("{} readback is out of range", fan.name),
                    format!("EC reports {cur:.1}%, which cannot be a percentage."),
                    "Treat speed as unknown; verify the NBFC model configuration.",
                ));
            } else if matches!((current, target), (Some(cur), Some(tgt)) if (cur - tgt).abs() > 20.0)
            {
                warnings.push(Warning::new(
                    format!("FAN_{}_READBACK_IMPLAUSIBLE", fan.index),
                    Severity::Warning,
                    format!("{} EC readback is not conclusive", fan.name),
                    format!(
                        "EC reports {:.1}% against a target of {:.1}%; EC data is not RPM.",
                        current.unwrap_or(0.0),
                        target.unwrap_or(0.0)
                    ),
                    "Do not treat the reported speed as measured evidence; check RPM availability.",
                ));
            }
        }

        let sensor_config = view.sensor_config.as_ref();
        for fan in &status.fans {
            if fan_role(&fan.name) == crate::policy::FanRole::Gpu {
                let source = sensor_config
                    .and_then(|cfg| cfg.fans.iter().find(|f| f.index == fan.index))
                    .map(|f| f.sensors.clone())
                    .unwrap_or_default();
                let has_gpu = source.iter().any(|s| {
                    let lower = s.to_ascii_lowercase();
                    lower.contains("gpu")
                        || lower.contains("amdgpu")
                        || lower.contains("nvidia")
                        || lower.contains("nouveau")
                        || lower.contains("radeon")
                });
                if !has_gpu {
                    warnings.push(Warning::new(
                        "GPU_FAN_USES_CPU_SENSOR",
                        Severity::Warning,
                        "GPU fan is not driven by a GPU sensor",
                        format!("{} uses {:?}; GPU temperature is not used by NBFC.", fan.name, source),
                        "Keep automatic control; manual GPU control is blocked until the mapping is fixed and validated.",
                    ));
                }
            }
        }

        if let Some(selected) = status.selected_config.as_deref()
            && let Ok(report) = crate::nbfc_config::identity_report(selected, sensor_config)
        {
            warnings.extend(from_identity(&report));
        }

        if view
            .capabilities
            .as_ref()
            .map(|caps| !caps.protected_manual)
            .unwrap_or(true)
        {
            warnings.push(Warning::new(
                "NBFC_MANUAL_UNSUPPORTED",
                Severity::Info,
                "Protected manual control unavailable",
                "This NBFC version cannot provide the required JSON status/reset guarantees.",
                "Use `fan auto`; do not bypass the supervisor.",
            ));
        }
    }

    if let Some(supervisor) = supervisor {
        match supervisor.state {
            SupervisorState::Healthy | SupervisorState::Connected => {}
            SupervisorState::Degraded => warnings.push(Warning::new(
                "SUPERVISOR_DEGRADED",
                Severity::Warning,
                "Supervisor is degraded",
                supervisor
                    .reason
                    .clone()
                    .unwrap_or_else(|| "The supervisor reports a degraded state.".to_string()),
                SupervisorState::Degraded.action(),
            )),
            SupervisorState::NotRunning => warnings.push(Warning::new(
                "SUPERVISOR_NOT_RUNNING",
                Severity::Warning,
                "Supervisor is not running",
                supervisor
                    .error
                    .clone()
                    .unwrap_or_else(|| "The supervisor socket does not exist.".to_string()),
                SupervisorState::NotRunning.action(),
            )),
            SupervisorState::PermissionDenied => warnings.push(Warning::new(
                "SUPERVISOR_PERMISSION_DENIED",
                Severity::Warning,
                "Supervisor socket access denied",
                supervisor
                    .error
                    .clone()
                    .unwrap_or_else(|| "The current user cannot access the socket.".to_string()),
                SupervisorState::PermissionDenied.action(),
            )),
            SupervisorState::StaleSocket => warnings.push(Warning::new(
                "SUPERVISOR_STALE_SOCKET",
                Severity::Warning,
                "Supervisor socket is stale",
                supervisor
                    .error
                    .clone()
                    .unwrap_or_else(|| "The socket exists but no supervisor answers.".to_string()),
                SupervisorState::StaleSocket.action(),
            )),
            SupervisorState::ProtocolError => warnings.push(Warning::new(
                "SUPERVISOR_PROTOCOL_ERROR",
                Severity::Critical,
                "Supervisor IPC protocol error",
                supervisor
                    .error
                    .clone()
                    .unwrap_or_else(|| "The supervisor did not return a valid status.".to_string()),
                SupervisorState::ProtocolError.action(),
            )),
        }
    }

    // A stale sample is already rejected by the policy, but it deserves an
    // explicit user-facing warning when it appears in a snapshot.
    if view
        .snapshot
        .sensors
        .iter()
        .any(|s| s.status == SensorStatus::Unavailable)
    {
        warnings.push(Warning::new(
            "SENSOR_UNAVAILABLE",
            Severity::Warning,
            "Some sensors are unavailable",
            "Unavailable channels remain unavailable; they are never replaced by zero.",
            "Inspect `neosense sensors list` and `neosense diagnose --json`.",
        ));
    }

    warnings.sort_by_key(|warning| match warning.severity {
        Severity::Critical => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
        Severity::Success => 3,
    });
    warnings
}

pub fn highest_severity(warnings: &[Warning]) -> Option<Severity> {
    warnings.iter().map(|w| w.severity).max_by_key(|s| match s {
        Severity::Info => 0,
        Severity::Success => 1,
        Severity::Warning => 2,
        Severity::Critical => 3,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SensorSnapshot;
    use crate::nbfc::{NbfcFan, NbfcStatus};
    use crate::service::ServiceStatus;

    fn view() -> RuntimeView {
        RuntimeView {
            snapshot: SensorSnapshot::empty(),
            version: None,
            capabilities: None,
            status: Some(NbfcStatus {
                version: None,
                read_only: Some(false),
                selected_config: Some("test".into()),
                fans: vec![NbfcFan {
                    index: 0,
                    name: "GPU fan".into(),
                    temperature: Some(50.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(true),
                    critical: Some(false),
                    current_speed: Some(100.0),
                    target_speed: Some(40.0),
                    requested_speed: Some(-1.0),
                    speed_steps: Some(100),
                }],
                warnings: Vec::new(),
            }),
            service: ServiceStatus {
                active_state: Some("active".into()),
                ..ServiceStatus::default()
            },
            sensor_config: None,
            backend_error: None,
            supervisor: crate::supervisor::probe(std::path::Path::new(
                "/nonexistent-neosense-test.sock",
            )),
        }
    }

    #[test]
    fn missing_cpu_and_gpu_mapping_are_critical_or_warning() {
        let warnings = from_view(&view(), None);
        assert!(warnings.iter().any(|w| w.code == "CPU_SENSOR_UNAVAILABLE"));
        assert!(warnings.iter().any(|w| w.code == "GPU_FAN_USES_CPU_SENSOR"));
        assert!(!warnings.iter().any(|w| w.code == "SUPERVISOR_UNAVAILABLE"));
    }

    #[test]
    fn supervisor_states_produce_distinct_warnings() {
        let view = view();
        let missing =
            crate::supervisor::probe(std::path::Path::new("/nonexistent-neosense-warning.sock"));
        let warnings = from_view(&view, Some(&missing));
        assert!(warnings.iter().any(|w| w.code == "SUPERVISOR_NOT_RUNNING"));

        let permission = crate::supervisor::SupervisorProbe {
            state: crate::supervisor::SupervisorState::PermissionDenied,
            socket: std::path::PathBuf::from("/run/neosense/neosensed.sock"),
            socket_exists: true,
            socket_mode: Some("660".into()),
            socket_uid: Some(0),
            socket_gid: Some(964),
            health: None,
            reason: None,
            error: Some("socket access denied".into()),
            status: None,
        };
        let warnings = from_view(&view, Some(&permission));
        assert!(
            warnings
                .iter()
                .any(|w| w.code == "SUPERVISOR_PERMISSION_DENIED")
        );
        assert!(!warnings.iter().any(|w| w.code == "SUPERVISOR_NOT_RUNNING"));
    }

    #[test]
    fn severity_is_ordered_for_ui() {
        let severity = highest_severity(&[
            Warning::new("a", Severity::Info, "a", "a", "a"),
            Warning::new("b", Severity::Critical, "b", "b", "b"),
            Warning::new("c", Severity::Warning, "c", "c", "c"),
        ]);
        assert_eq!(severity, Some(Severity::Critical));
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use crate::nbfc_config::{FanIdentity, HardwareIdentityReport};
    use crate::policy::FanRole;

    fn identity(readback_configured: bool) -> HardwareIdentityReport {
        HardwareIdentityReport {
            schema: "neosense.hardware_identity/v1",
            selected_config: "Test".into(),
            config_path: "/tmp/Test.json".into(),
            notebook_model: "Test".into(),
            author: "Test".into(),
            critical_temperature: Some(95.0),
            ec_poll_interval_ms: Some(1000),
            read_write_words: Some(true),
            fans: vec![FanIdentity {
                index: 0,
                name: "CPU fan".into(),
                role: FanRole::Cpu,
                read_register: Some(19),
                write_register: Some(55),
                register_pair: "read=19 write=55".into(),
                min_speed_value: Some(0.0),
                max_speed_value: Some(100.0),
                min_speed_value_read: Some(0.0),
                max_speed_value_read: if readback_configured {
                    Some(100.0)
                } else {
                    Some(0.0)
                },
                independent_read_min_max_values: Some(false),
                readback_configured,
                reset_required: Some(true),
                fan_speed_reset_value: Some(50.0),
                sensor_sources: vec!["@CPU".into()],
                algorithm: Some("Average".into()),
                threshold_count: 1,
                physical_identity_verified: false,
            }],
            register_write_count: 0,
            warnings: if readback_configured {
                Vec::new()
            } else {
                vec!["no valid readback range".into()]
            },
            hardware_applied: false,
        }
    }

    #[test]
    fn invalid_readback_range_produces_a_concrete_warning() {
        let warnings = from_identity(&identity(false));
        assert!(warnings.iter().any(|warning| {
            warning.code == "NBFC_READBACK_CONFIG_INVALID" && warning.detail.contains("CPU fan")
        }));
    }

    #[test]
    fn valid_readback_range_does_not_produce_the_invalid_range_warning() {
        let warnings = from_identity(&identity(true));
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.code == "NBFC_READBACK_CONFIG_INVALID")
        );
    }

    #[test]
    fn insecure_socket_produces_a_security_warning() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nbfc.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let security = crate::nbfc::inspect_socket_with_identity(
            &path,
            &crate::nbfc::SupervisorIdentity::current(),
        );
        let warning = from_socket_security(&security).unwrap();
        assert_eq!(warning.code, "NBFC_SOCKET_INSECURE");
        assert!(warning.detail.contains("bypass"));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let security = crate::nbfc::inspect_socket_with_identity(
            &path,
            &crate::nbfc::SupervisorIdentity::current(),
        );
        assert!(from_socket_security(&security).is_none());
    }
}
