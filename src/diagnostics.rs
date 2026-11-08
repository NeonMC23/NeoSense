use crate::daemon::socket_path_from_env;
use crate::ipc::Client;
use crate::model::{SensorSnapshot, Trust, now_ms};
use crate::nbfc::{NbfcCapabilities, NbfcCli, NbfcSensorConfig, NbfcStatus};
use crate::policy::{FanRole, SafetyConfig, assess_manual_request, fan_role};
use crate::service::{ServiceStatus, is_unit_active, nbfc_service_status};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCheck {
    pub id: String,
    pub title: String,
    pub status: CheckStatus,
    pub message: String,
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
    pub generated_at_ms: u64,
    pub environment: Environment,
    pub nbfc_version: Option<String>,
    pub capabilities: Option<NbfcCapabilities>,
    pub service: ServiceStatus,
    pub selected_config: Option<String>,
    pub sensors: SensorSnapshot,
    pub checks: Vec<DiagnosticCheck>,
}

impl DiagnosticReport {
    pub fn overall(&self) -> CheckStatus {
        if self.checks.iter().any(|c| c.status == CheckStatus::Fail) {
            CheckStatus::Fail
        } else if self.checks.iter().any(|c| c.status == CheckStatus::Warning) {
            CheckStatus::Warning
        } else {
            CheckStatus::Ok
        }
    }
}

pub fn diagnose(nbfc: &NbfcCli, snapshot: &SensorSnapshot) -> DiagnosticReport {
    let environment = environment();
    let service = nbfc_service_status();
    let now = now_ms();
    let config = SafetyConfig::default();

    let mut checks = Vec::new();
    let version = match nbfc.version() {
        Ok(v) => {
            let caps = NbfcCapabilities::from_version(v);
            checks.push(check(
                "nbfc.version",
                "NBFC version",
                if caps.protected_manual {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warning
                },
                if caps.protected_manual {
                    format!("NBFC {v}: protected manual control available (JSON, reset-ec)")
                } else {
                    format!("NBFC {v}: protected manual control unavailable; use NBFC >= 0.5.4")
                },
            ));
            Some((v, caps))
        }
        Err(e) => {
            checks.push(check("nbfc.version", "NBFC version", CheckStatus::Fail, e));
            None
        }
    };

    let status = match &version {
        Some((v, _)) => match nbfc.status(*v) {
            Ok(s) => {
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
                Some(s)
            }
            Err(e) => {
                checks.push(check("nbfc.service", "NBFC service", CheckStatus::Fail, e));
                None
            }
        },
        None => None,
    };

    if let Some(status) = &status {
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

    if let Some(status) = &status {
        match nbfc.sensors_show() {
            Ok(sensor_config) => {
                if let Some((_, capabilities)) = &version {
                    check_fan_sources(
                        &mut checks,
                        status,
                        &sensor_config,
                        capabilities,
                        &config,
                        snapshot,
                        now,
                    );
                }
            }
            Err(e) => checks.push(check(
                "nbfc.sensor_sources",
                "NBFC sensor sources",
                CheckStatus::Warning,
                format!("nbfc sensors show unavailable: {e}"),
            )),
        }
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
    if snapshot
        .sensors
        .iter()
        .all(|s| !matches!(s.role, crate::model::SensorRole::FanRpm(_)))
    {
        checks.push(check(
            "sensor.rpm",
            "Fan RPM",
            CheckStatus::Info,
            "no readable tachometer; physical verification is limited".to_string(),
        ));
    }

    if let Ok(metadata) = std::fs::metadata("/run/nbfc_service.socket") {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o777;
        checks.push(check(
            "security.nbfc_socket",
            "NBFC socket permissions",
            if mode & 0o002 != 0 {
                CheckStatus::Warning
            } else {
                CheckStatus::Info
            },
            format!(
                "socket /run/nbfc_service.socket mode {mode:o}; broad local access bypasses NeoSense"
            ),
        ));
    }

    let supervisor_socket = socket_path_from_env();
    match Client::connect(&supervisor_socket) {
        Ok(mut client) => match client.call("status.get", serde_json::json!({})) {
            Ok(value) => {
                let healthy = value
                    .get("health")
                    .and_then(|health| health.get("healthy"))
                    .and_then(|healthy| healthy.as_bool())
                    .unwrap_or(false);
                let reason = value
                    .get("health")
                    .and_then(|health| health.get("reason"))
                    .and_then(|reason| reason.as_str())
                    .unwrap_or("");
                checks.push(check(
                    "supervisor.ipc",
                    "NeoSense supervisor",
                    if healthy {
                        CheckStatus::Ok
                    } else {
                        CheckStatus::Warning
                    },
                    if healthy {
                        format!("healthy supervisor on {}", supervisor_socket.display())
                    } else {
                        format!("degraded supervisor: {reason}")
                    },
                ));
            }
            Err(error) => checks.push(check(
                "supervisor.ipc",
                "NeoSense supervisor",
                CheckStatus::Warning,
                format!(
                    "socket reachable but IPC status unavailable: {}",
                    error.message
                ),
            )),
        },
        Err(error) => checks.push(check(
            "supervisor.ipc",
            "NeoSense supervisor",
            if error.code == "permission_denied" {
                CheckStatus::Warning
            } else {
                CheckStatus::Info
            },
            format!(
                "supervisor inaccessible ({}): {}",
                supervisor_socket.display(),
                error.message
            ),
        )),
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

    DiagnosticReport {
        generated_at_ms: now,
        environment,
        nbfc_version: version.as_ref().map(|(v, _)| v.to_string()),
        capabilities: version.as_ref().map(|(_, c)| c.clone()),
        service,
        selected_config: status.as_ref().and_then(|s| s.selected_config.clone()),
        sensors: snapshot.clone(),
        checks,
    }
}

#[allow(clippy::too_many_arguments)]
fn check_fan_sources(
    checks: &mut Vec<DiagnosticCheck>,
    status: &NbfcStatus,
    sensor_config: &NbfcSensorConfig,
    capabilities: &NbfcCapabilities,
    config: &SafetyConfig,
    snapshot: &SensorSnapshot,
    now: u64,
) {
    for fan in &status.fans {
        let source = sensor_config
            .fans
            .iter()
            .find(|f| f.index == fan.index)
            .map(|f| f.sensors.clone())
            .unwrap_or_default();
        let role = fan_role(&fan.name);
        if role == FanRole::Gpu {
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

        let decision = assess_manual_request(
            snapshot,
            status,
            capabilities,
            Some(sensor_config),
            fan.index,
            50.0,
            config,
            now,
        );
        checks.push(check(
            format!("manual.{}", fan.index),
            "Manual-control eligibility",
            CheckStatus::Info,
            if decision.allowed {
                format!("{}: manual control allowed by policy", fan.name)
            } else {
                format!(
                    "{}: manual control refused: {}",
                    fan.name,
                    decision.reasons.join("; ")
                )
            },
        ));
    }
}

fn check(
    id: impl Into<String>,
    title: impl Into<String>,
    status: CheckStatus,
    message: impl Into<String>,
) -> DiagnosticCheck {
    DiagnosticCheck {
        id: id.into(),
        title: title.into(),
        status,
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
