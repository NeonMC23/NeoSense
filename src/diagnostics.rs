use crate::control::{
    ControlMode, ControlReadiness, ControlStage, assess_view as assess_control_view,
};
use crate::model::{SensorSnapshot, Trust, now_ms};
use crate::nbfc::{NbfcCapabilities, NbfcSensorConfig, NbfcStatus};
use crate::nbfc_config;
use crate::policy::{FanRole, SafetyConfig, fan_role};
use crate::runtime::RuntimeView;
use crate::service::{ServiceStatus, is_unit_active};
use crate::supervisor::{SupervisorProbe, SupervisorState};
use crate::util::run_command;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Info,
    Warning,
    Fail,
}

impl CheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Ok => "OK",
            CheckStatus::Info => "INFO",
            CheckStatus::Warning => "WARN",
            CheckStatus::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckGroup {
    Backend,
    Hardware,
    Thermal,
    Fans,
    Control,
    Security,
}

impl CheckGroup {
    pub fn label(self) -> &'static str {
        match self {
            CheckGroup::Backend => "Backend",
            CheckGroup::Hardware => "Hardware",
            CheckGroup::Thermal => "Thermal monitoring",
            CheckGroup::Fans => "Fan telemetry",
            CheckGroup::Control => "Control capabilities",
            CheckGroup::Security => "Security and permissions",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCheck {
    pub id: String,
    pub title: String,
    pub status: CheckStatus,
    pub group: CheckGroup,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticSummary {
    pub overall: CheckStatus,
    pub operational: bool,
    pub supervisor: SupervisorState,
    /// The protected manual path is policy-eligible (backend version, access
    /// control, CPU sensor and fan data). This is not evidence of physical
    /// control.
    pub fan_control_available: bool,
    /// A physical fan tachometer/RPM channel is present.
    pub physical_feedback_available: bool,
    /// At least one RPM channel has a current, readable value.
    #[serde(default)]
    pub physical_feedback_fresh: bool,
    /// At least one RPM channel is a valid, fresh, trusted physical feedback
    /// source. This is never regulation or control eligibility.
    #[serde(default)]
    pub physical_feedback_trusted: bool,
    /// A verified CPU/GPU physical mapping exists for the RPM channels.
    #[serde(default)]
    pub physical_identity_verified: bool,
    /// A validated physical-control observation exists for this machine.
    /// NeoSense never infers this from backend acceptance.
    pub physical_control_validated: bool,
    /// Structured control-readiness stage. `hardware_validated` requires
    /// explicit physical-identity, RPM-calibration and actuator-response
    /// evidence, not merely a responsive backend.
    #[serde(default)]
    pub control_stage: ControlStage,
    #[serde(default)]
    pub control_mode: ControlMode,
    /// Every configured fan has a policy-eligible manual path.
    #[serde(default)]
    pub all_fans_eligible: bool,
    pub thermal_reliable: bool,
    pub critical_issues: usize,
    pub warning_issues: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub bios: Option<String>,
    pub virtualization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub schema: &'static str,
    pub generated_at_ms: u64,
    pub environment: Environment,
    pub nbfc_version: Option<String>,
    pub capabilities: Option<NbfcCapabilities>,
    pub service: ServiceStatus,
    pub selected_config: Option<String>,
    pub supervisor: SupervisorProbe,
    pub sensors: SensorSnapshot,
    /// Why no supervisor sensor snapshot was available. Clients never fall
    /// back to reading hardware directly.
    #[serde(default)]
    pub sensor_source_error: Option<String>,
    /// Structured, read-only control-readiness assessment.
    pub control: ControlReadiness,
    pub summary: DiagnosticSummary,
    pub checks: Vec<DiagnosticCheck>,
}

impl DiagnosticReport {
    pub fn overall(&self) -> CheckStatus {
        self.summary.overall
    }

    pub fn checks_in(&self, group: CheckGroup) -> impl Iterator<Item = &DiagnosticCheck> {
        self.checks.iter().filter(move |check| check.group == group)
    }
}

/// Build a diagnostic report from the supervisor-provided runtime view.
///
/// This never connects to NBFC from the CLI/TUI process. A root-only NBFC
/// socket means an ordinary user cannot connect; reporting that as a backend
/// outage would be wrong, so backend state is always taken from the supervisor
/// that actually holds the privilege.
pub fn diagnose(view: &RuntimeView) -> DiagnosticReport {
    let snapshot = &view.snapshot;
    let environment = environment();
    let service = view.service.clone();
    let now = now_ms();
    let config = SafetyConfig::default();
    let supervisor = view.supervisor.clone();

    let mut checks = Vec::new();
    let capabilities = view.version.map(|version| {
        view.capabilities
            .clone()
            .unwrap_or_else(|| NbfcCapabilities::from_version(version))
    });

    let version = match (view.version, capabilities) {
        (Some(version), Some(capabilities)) => {
            checks.push(check(
                "nbfc.version",
                "NBFC version",
                if capabilities.protected_manual {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warning
                },
                if capabilities.protected_manual {
                    format!("NBFC {version}: protected manual control available (JSON, reset-ec)")
                } else {
                    format!(
                        "NBFC {version}: protected manual control unavailable; use NBFC >= 0.5.4"
                    )
                },
            ));
            Some((version, capabilities))
        }
        _ => {
            let message = view.backend_error.clone().unwrap_or_else(|| {
                if supervisor.state.is_connected() {
                    "supervisor connected but reported no NBFC version".to_string()
                } else {
                    format!(
                        "supervisor {}: backend state unavailable; this process does not access NBFC directly",
                        supervisor.state.label()
                    )
                }
            });
            checks.push(check(
                "nbfc.version",
                "NBFC version",
                CheckStatus::Fail,
                message,
            ));
            None
        }
    };

    checks.push(check(
        "nbfc.service",
        "NBFC service",
        if service.is_active() {
            CheckStatus::Ok
        } else {
            CheckStatus::Fail
        },
        match (&service.active_state, &service.error) {
            (Some(state), _) => format!("nbfc_service: {state}"),
            (None, Some(e)) => e.clone(),
            _ => "service state unavailable".to_string(),
        },
    ));

    if version.is_some() && view.status.is_none() {
        checks.push(check(
            "nbfc.status",
            "NBFC status query",
            CheckStatus::Fail,
            view.backend_error
                .clone()
                .unwrap_or_else(|| "supervisor could not read NBFC status".to_string()),
        ));
    }

    let status = view.status.as_ref();

    if let Some(status) = status {
        if status.selected_config.is_some() {
            checks.push(check(
                "nbfc.config",
                "NBFC configuration",
                CheckStatus::Ok,
                status.selected_config.clone().unwrap_or_default(),
            ));
        } else {
            checks.push(check(
                "nbfc.config",
                "NBFC configuration",
                CheckStatus::Warning,
                "no configuration selected".to_string(),
            ));
        }
        checks.push(check(
            "nbfc.fans",
            "NBFC fans",
            if status.fans.is_empty() {
                CheckStatus::Fail
            } else {
                CheckStatus::Ok
            },
            format!("{} fan(s) detected", status.fans.len()),
        ));
        for fan in &status.fans {
            let readback = match (fan.current_speed, fan.target_speed) {
                (Some(cur), _) if !(0.0..=100.0).contains(&cur) => check(
                    format!("fan.{}.readback", fan.index),
                    "Fan telemetry",
                    CheckStatus::Fail,
                    format!(
                        "{}: EC speed {cur:.2}% out of range; not trustworthy",
                        fan.name
                    ),
                ),
                (Some(cur), Some(target)) if (cur - target).abs() > 20.0 => check(
                    format!("fan.{}.readback", fan.index),
                    "Fan telemetry",
                    CheckStatus::Warning,
                    format!(
                        "{}: EC speed {cur:.1}% differs greatly from target {target:.1}%; inconclusive (EC is not RPM)",
                        fan.name
                    ),
                ),
                (Some(cur), Some(target)) => check(
                    format!("fan.{}.readback", fan.index),
                    "Fan telemetry",
                    CheckStatus::Info,
                    format!(
                        "{}: EC {cur:.1}% / target {target:.1}%; no RPM measurement to confirm",
                        fan.name
                    ),
                ),
                _ => check(
                    format!("fan.{}.readback", fan.index),
                    "Fan telemetry",
                    CheckStatus::Info,
                    format!("{}: EC telemetry incomplete", fan.name),
                ),
            };
            checks.push(readback);
        }
    }

    let cpu = snapshot.cpu_package(now, config.stale_after_ms);
    checks.push(match cpu {
        Some(sensor) => check(
            "sensor.cpu",
            "CPU temperature",
            if sensor.health.trust.is_at_least(Trust::Trusted) {
                CheckStatus::Ok
            } else {
                CheckStatus::Warning
            },
            format!("{} ({})", sensor.display_value(), sensor.role),
        ),
        None => check(
            "sensor.cpu",
            "CPU temperature",
            CheckStatus::Fail,
            "no valid, fresh CPU measurement".to_string(),
        ),
    });

    let gpus = snapshot.gpu_indexes();
    if gpus.is_empty() {
        checks.push(check(
            "sensor.gpu",
            "GPU temperature",
            CheckStatus::Warning,
            "no GPU sensor identified (iGPU or driver not exposed)".to_string(),
        ));
    } else {
        for index in gpus {
            let sensor = snapshot.gpu_die(index, now, config.stale_after_ms);
            checks.push(check(
                format!("sensor.gpu.{index}"),
                "GPU temperature",
                if sensor.is_some() {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warning
                },
                sensor
                    .map(|s| format!("GPU {index}: {}", s.display_value()))
                    .unwrap_or_else(|| format!("GPU {index}: measurement unavailable")),
            ));
        }
    }

    if let Some((_, capabilities)) = &version {
        checks.push(check(
            "manual.global",
            "Manual control",
            if capabilities.protected_manual {
                CheckStatus::Info
            } else {
                CheckStatus::Warning
            },
            if capabilities.protected_manual {
                "Backend exposes protected manual control; each request still requires fresh sensors, mapping and confirmation."
                    .to_string()
            } else {
                "Protected manual control is unavailable on this NBFC version; one backend limitation, not one per fan."
                    .to_string()
            },
        ));
    }

    if let Some(status) = status {
        match &view.sensor_config {
            Some(sensor_config) => {
                check_fan_sources(&mut checks, status, sensor_config);
                if let Some(config_name) = status.selected_config.as_deref() {
                    match nbfc_config::identity_report(config_name, Some(sensor_config)) {
                        Ok(report) => {
                            checks.push(check(
                                "nbfc.fan.identity",
                                "Fan identity evidence",
                                CheckStatus::Info,
                                format!(
                                    "{} logical fan channel(s) in '{}': {}",
                                    report.fans.len(),
                                    report.selected_config,
                                    report
                                        .fans
                                        .iter()
                                        .map(|fan| format!("{} [{}]", fan.name, fan.register_pair))
                                        .collect::<Vec<_>>()
                                        .join("; ")
                                ),
                            ));
                            for fan in &report.fans {
                                let readback = match fan.readback_range_source {
                                    crate::nbfc_config::ReadbackRangeSource::Independent => {
                                        format!(
                                            "independent read scale {}..{}",
                                            bound(fan.effective_read_min),
                                            bound(fan.effective_read_max)
                                        )
                                    }
                                    crate::nbfc_config::ReadbackRangeSource::WriteRange => format!(
                                        "read scale inferred from the write range {}..{} (IndependentReadMinMaxValues=false); NBFC clamps out-of-range raw values, so CurrentSpeed is not a measurement",
                                        bound(fan.effective_read_min),
                                        bound(fan.effective_read_max)
                                    ),
                                    crate::nbfc_config::ReadbackRangeSource::Invalid => {
                                        "no usable read range".to_string()
                                    }
                                };
                                checks.push(check(
                                    format!("nbfc.fan.{}.registers", fan.index),
                                    "Fan register mapping",
                                    if fan.readback_range_source
                                        == crate::nbfc_config::ReadbackRangeSource::Independent
                                    {
                                        CheckStatus::Ok
                                    } else {
                                        CheckStatus::Warning
                                    },
                                    format!(
                                        "{}: role {}, {}, sensors {}; {}",
                                        fan.name,
                                        fan.role,
                                        fan.register_pair,
                                        if fan.sensor_sources.is_empty() {
                                            "none".to_string()
                                        } else {
                                            fan.sensor_sources.join(", ")
                                        },
                                        readback
                                    ),
                                ));
                            }
                            for warning in &report.warnings {
                                if warning.contains("physical") || warning.contains("tachometer") {
                                    checks.push(check(
                                        "nbfc.fan.identity_limits",
                                        "Fan identity limits",
                                        CheckStatus::Warning,
                                        warning.clone(),
                                    ));
                                }
                            }
                        }
                        Err(error) => checks.push(check(
                            "nbfc.fan.identity",
                            "Fan identity evidence",
                            CheckStatus::Warning,
                            error,
                        )),
                    }
                }
            }
            None => checks.push(check(
                "nbfc.sensor_sources",
                "NBFC sensor sources",
                CheckStatus::Warning,
                "NBFC sensor sources are unavailable from the supervisor".to_string(),
            )),
        }
    }

    if let Some(error) = &view.sensor_source_error {
        checks.push(check(
            "sensor.source",
            "Sensor snapshot source",
            CheckStatus::Warning,
            error.clone(),
        ));
    }
    for error in &snapshot.errors {
        checks.push(check(
            format!("provider.{}", error.provider),
            "Sensor provider",
            CheckStatus::Warning,
            error.message.clone(),
        ));
    }
    for conflict in snapshot.conflicts() {
        checks.push(check(
            "sensor.conflict",
            "Sensor conflict",
            CheckStatus::Warning,
            conflict,
        ));
    }
    let rpm_sensors: Vec<&crate::model::SensorSample> = snapshot.rpm_sensors().collect();
    if rpm_sensors.is_empty() {
        checks.push(check(
            "sensor.rpm",
            "Fan RPM",
            CheckStatus::Info,
            "no independent tachometer; physical verification is limited".to_string(),
        ));
    } else {
        for sensor in &rpm_sensors {
            let index = match sensor.role {
                crate::model::SensorRole::FanRpm(index) => index,
                _ => 0,
            };
            let fresh = sensor.value.is_some() && sensor.is_fresh(now, config.stale_after_ms);
            let feedback = sensor.is_usable_for_feedback(now, config.stale_after_ms);
            checks.push(check(
                format!("sensor.rpm.{index}"),
                "Fan RPM feedback",
                if feedback {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warning
                },
                format!(
                    "{}: {} ({}; {}); physical measurement, CPU/GPU identity unverified",
                    sensor.display_name(),
                    sensor.display_value(),
                    sensor.health.trust.short_label(),
                    if feedback {
                        "fresh, trusted feedback"
                    } else if fresh {
                        "fresh but not feedback-eligible"
                    } else {
                        "stale or unavailable"
                    }
                ),
            ));
        }
        checks.push(check(
            "feedback.identity",
            "RPM fan identity",
            CheckStatus::Warning,
            "Acer hwmon fan channels are physical measurements, but CPU/GPU fan identity is not verified"
                .to_string(),
        ));
        checks.push(check(
            "feedback.regulation",
            "RPM regulation eligibility",
            CheckStatus::Info,
            "RPM feedback is never eligible for temperature regulation".to_string(),
        ));
    }

    // Prefer the supervisor's own inspection: its credentials are the ones
    // that matter, and an ordinary CLI process cannot validate root access.
    let (nbfc_socket, socket_identity_verified) = match view
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.socket.clone())
    {
        Some(socket) => (socket, true),
        None => (crate::nbfc::inspect_default_socket(), false),
    };
    let socket_message = if !nbfc_socket.exists {
        format!(
            "socket {} is not present; access cannot be classified",
            nbfc_socket.path
        )
    } else if !socket_identity_verified {
        format!(
            "socket {} mode {} uid {:?} gid {:?}; permissions were inspected locally, but supervisor access cannot be verified because the supervisor is not connected",
            nbfc_socket.path,
            nbfc_socket.mode.as_deref().unwrap_or("unknown"),
            nbfc_socket.uid,
            nbfc_socket.gid
        )
    } else {
        let mode = nbfc_socket.mode.as_deref().unwrap_or("unknown");
        if nbfc_socket.access_controlled {
            format!(
                "socket {} mode {mode} uid {:?} gid {:?}; no group/world write access detected. Supervisor identity source: {}.",
                nbfc_socket.path, nbfc_socket.uid, nbfc_socket.gid, nbfc_socket.identity_source
            )
        } else {
            format!(
                "SECURITY: socket {} mode {mode} uid {:?} gid {:?}; {}. Supervisor identity: {}. No safe permission workaround was applied.",
                nbfc_socket.path,
                nbfc_socket.uid,
                nbfc_socket.gid,
                nbfc_socket
                    .error
                    .clone()
                    .unwrap_or_else(|| "access control is not established".to_string()),
                nbfc_socket.identity_source
            )
        }
    };
    checks.push(check(
        "security.nbfc_socket",
        "NBFC socket permissions",
        if !nbfc_socket.exists {
            CheckStatus::Info
        } else if !socket_identity_verified {
            CheckStatus::Warning
        } else if nbfc_socket.access_controlled {
            CheckStatus::Ok
        } else {
            CheckStatus::Warning
        },
        socket_message,
    ));

    let (supervisor_status, supervisor_detail) = match supervisor.state {
        SupervisorState::Healthy | SupervisorState::Connected => {
            (CheckStatus::Ok, supervisor.detail())
        }
        SupervisorState::Degraded => (CheckStatus::Warning, supervisor.detail()),
        SupervisorState::NotRunning => (CheckStatus::Info, supervisor.detail()),
        SupervisorState::PermissionDenied => (CheckStatus::Warning, supervisor.detail()),
        SupervisorState::StaleSocket => (CheckStatus::Warning, supervisor.detail()),
        SupervisorState::ProtocolError => (CheckStatus::Fail, supervisor.detail()),
    };
    let mut supervisor_detail = supervisor_detail;
    if let Some(reason) = &supervisor.reason {
        supervisor_detail.push_str(" — ");
        supervisor_detail.push_str(reason);
    }
    checks.push(check(
        "supervisor.ipc",
        "NeoSense supervisor",
        supervisor_status,
        supervisor_detail,
    ));

    let custom = supervisor
        .status
        .as_ref()
        .and_then(|status| status.get("custom"))
        .cloned();
    if let Some(custom) = custom {
        let path = custom
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("path unavailable");
        let count = custom
            .get("profiles")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if let Some(error) = custom.get("error").and_then(serde_json::Value::as_str) {
            checks.push(check(
                "config.custom_profiles",
                "Custom profile store",
                CheckStatus::Fail,
                format!("{path}: {error}"),
            ));
        } else if let Some(warning) = custom
            .get("warnings")
            .and_then(serde_json::Value::as_array)
            .and_then(|warnings| warnings.first())
            .and_then(serde_json::Value::as_str)
        {
            checks.push(check(
                "config.custom_profiles",
                "Custom profile store",
                CheckStatus::Warning,
                format!("{path}: {warning}"),
            ));
        } else {
            checks.push(check(
                "config.custom_profiles",
                "Custom profile store",
                CheckStatus::Ok,
                format!("{count} custom curve(s) at {path}; simulation-only"),
            ));
        }
    } else {
        let local = crate::custom_profiles::Store::client().status();
        checks.push(check(
            "config.custom_profiles",
            "Custom profile store",
            if local.error.is_some() {
                CheckStatus::Warning
            } else {
                CheckStatus::Info
            },
            local.error.unwrap_or_else(|| {
                format!(
                    "{} custom curve(s) in the unprivileged fallback store at {}; no supervisor store reported",
                    local.profiles.len(),
                    local.path
                )
            }),
        ));
    }

    for unit in ["fancontrol", "thinkfan", "coolercontrol"] {
        if is_unit_active(unit) == Some(true) {
            checks.push(check(
                format!("conflict.{unit}"),
                "Conflicting controller",
                CheckStatus::Warning,
                format!("{unit} is active; possible control conflict"),
            ));
        }
    }
    for unit in ["power-profiles-daemon", "tuned", "thermald"] {
        if is_unit_active(unit) == Some(true) {
            checks.push(check(
                format!("platform.{unit}"),
                "Platform thermal/power policy",
                CheckStatus::Info,
                format!("{unit} is active; NeoSense did not modify it"),
            ));
        }
    }

    let critical_issues = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();
    let warning_issues = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Warning)
        .count();
    let overall = if critical_issues > 0 {
        CheckStatus::Fail
    } else if warning_issues > 0 {
        CheckStatus::Warning
    } else {
        CheckStatus::Ok
    };
    let thermal_reliable = cpu.is_some()
        && !checks
            .iter()
            .any(|check| check.group == CheckGroup::Thermal && check.status == CheckStatus::Fail);
    // Physical feedback requires an actual tachometer/RPM sensor. Backend
    // percentages and EC estimates are not physical feedback.
    let physical_feedback_available = !rpm_sensors.is_empty();
    let physical_feedback_fresh = rpm_sensors
        .iter()
        .any(|sensor| sensor.value.is_some() && sensor.is_fresh(now, config.stale_after_ms));
    let physical_feedback_trusted = rpm_sensors
        .iter()
        .any(|sensor| sensor.is_usable_for_feedback(now, config.stale_after_ms));
    let control = control_readiness(view);
    let fan_control_available = control.manual_path_eligible;
    let summary = DiagnosticSummary {
        overall,
        operational: critical_issues == 0,
        supervisor: supervisor.state,
        fan_control_available,
        physical_feedback_available,
        physical_feedback_fresh,
        physical_feedback_trusted,
        physical_identity_verified: control.physical_identity_verified,
        physical_control_validated: control.physical_control_validated,
        control_stage: control.stage,
        control_mode: control.mode,
        all_fans_eligible: control.all_fans_eligible,
        thermal_reliable,
        critical_issues,
        warning_issues,
    };

    DiagnosticReport {
        schema: "neosense.diagnose/v1",
        generated_at_ms: now,
        environment,
        nbfc_version: version.as_ref().map(|(v, _)| v.to_string()),
        capabilities: version.as_ref().map(|(_, c)| c.clone()),
        service,
        selected_config: status.and_then(|s| s.selected_config.clone()),
        supervisor,
        sensors: snapshot.clone(),
        sensor_source_error: view.sensor_source_error.clone(),
        control,
        summary,
        checks,
    }
}

/// Build the read-only control-readiness assessment used by both `diagnose`
/// and `status`. The implementation lives in `control::assess_view` so all
/// consumers share one state machine.
pub fn control_readiness(view: &RuntimeView) -> ControlReadiness {
    assess_control_view(view)
}

fn check_fan_sources(
    checks: &mut Vec<DiagnosticCheck>,
    status: &NbfcStatus,
    sensor_config: &NbfcSensorConfig,
) {
    for fan in &status.fans {
        let source = sensor_config
            .fans
            .iter()
            .find(|f| f.index == fan.index)
            .map(|f| f.sensors.clone())
            .unwrap_or_default();
        if fan_role(&fan.name) == FanRole::Gpu {
            let has_gpu = source.iter().any(|s| {
                let lower = s.to_ascii_lowercase();
                lower.contains("gpu")
                    || lower.contains("amdgpu")
                    || lower.contains("nvidia")
                    || lower.contains("nouveau")
                    || lower.contains("radeon")
            });
            checks.push(check(
                format!("nbfc.fan.{}.source", fan.index),
                "GPU fan sensor source",
                if has_gpu {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warning
                },
                if has_gpu {
                    format!("{} uses {source:?}", fan.name)
                } else {
                    format!(
                        "{} uses {source:?}; GPU temperature is not used by NBFC",
                        fan.name
                    )
                },
            ));
        }
    }
}

fn group_for(id: &str) -> CheckGroup {
    if id == "nbfc.fans"
        || id == "sensor.rpm"
        || id.starts_with("sensor.rpm.")
        || id.starts_with("feedback.")
        || id.starts_with("fan.")
    {
        CheckGroup::Fans
    } else if id.starts_with("nbfc.fan.")
        || id == "nbfc.sensor_sources"
        || id.starts_with("manual.")
        || id.starts_with("config.")
    {
        CheckGroup::Control
    } else if id == "nbfc.version"
        || id == "nbfc.service"
        || id == "nbfc.config"
        || id == "nbfc.status"
    {
        CheckGroup::Backend
    } else if id.starts_with("sensor.") || id.starts_with("provider.") {
        CheckGroup::Thermal
    } else if id.starts_with("security.")
        || id.starts_with("supervisor.")
        || id.starts_with("conflict.")
    {
        CheckGroup::Security
    } else {
        CheckGroup::Hardware
    }
}

fn bound(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.0}"))
        .unwrap_or_else(|| "?".to_string())
}

fn check(
    id: impl Into<String>,
    title: impl Into<String>,
    status: CheckStatus,
    message: impl Into<String>,
) -> DiagnosticCheck {
    let id = id.into();
    let group = group_for(&id);
    DiagnosticCheck {
        id,
        title: title.into(),
        status,
        group,
        message: message.into(),
    }
}

fn environment() -> Environment {
    Environment {
        os: os_release("PRETTY_NAME"),
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string()),
        vendor: read_sys("sys_vendor"),
        model: read_sys("product_name"),
        bios: read_sys("bios_version"),
        virtualization: run_command("systemd-detect-virt", &[], Duration::from_secs(1))
            .ok()
            .filter(|o| o.success)
            .map(|o| o.stdout.trim().to_string())
            .filter(|s| !s.is_empty() && s != "none"),
    }
}

fn read_sys(name: &str) -> Option<String> {
    let path = Path::new("/sys/class/dmi/id").join(name);
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn os_release(key: &str) -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix(&format!("{key}=")) {
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}
