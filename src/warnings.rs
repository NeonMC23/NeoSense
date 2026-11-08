//! Shared warning model for the CLI and TUI.
//!
//! Severity is textual as well as coloured: a warning must remain meaningful
//! without colour and must explain the cause, the safety consequence and the
//! next safe action.

use crate::model::SensorStatus;
use crate::policy::fan_role;
use crate::runtime::RuntimeView;
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

pub fn from_view(view: &RuntimeView, supervisor_connected: bool) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let now = crate::model::now_ms();
    let ttl = 5_000;

    if view.backend_error.is_some() || view.status.is_none() {
        warnings.push(Warning::new(
            "NBFC_UNAVAILABLE",
            Severity::Critical,
            "NBFC backend unavailable",
            "Fan state and automatic control cannot be verified.",
            "Check `nbfc_service` and `nbfc status`; do not attempt manual fan control.",
        ));
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

    if !supervisor_connected {
        warnings.push(Warning::new(
            "SUPERVISOR_UNAVAILABLE",
            Severity::Warning,
            "Supervisor is not connected",
            "Read-only monitoring is available; fan-control requests will be rejected.",
            "Start `neosensed` and verify socket permissions or group membership.",
        ));
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
        }
    }

    #[test]
    fn missing_cpu_and_gpu_mapping_are_critical_or_warning() {
        let warnings = from_view(&view(), false);
        assert!(warnings.iter().any(|w| w.code == "CPU_SENSOR_UNAVAILABLE"));
        assert!(warnings.iter().any(|w| w.code == "GPU_FAN_USES_CPU_SENSOR"));
        assert!(warnings.iter().any(|w| w.code == "SUPERVISOR_UNAVAILABLE"));
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
