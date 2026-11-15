//! Discovered logical fan inventory.
//!
//! This is a read-only projection of supervisor-provided backend state and the
//! existing control-readiness model. It never invents physical identity, RPM
//! mapping or temperature substitution.

use crate::control::{self, ControlCapabilityState};
use crate::model::{SensorRole, SensorSample, SensorSnapshot, Trust, now_ms};
use crate::nbfc::{NbfcCapabilities, NbfcFan, NbfcSensorConfig, NbfcStatus};
use crate::policy::{
    AutomaticModeVerification, FanRole, SafetyConfig, TemperatureMappingKind,
    assess_manual_request, automatic_mode_evidence, temperature_mapping,
};
use crate::runtime::RuntimeView;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "neosense.fan_inventory/v1";
pub const FAN_SCHEMA: &str = "neosense.fan/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalIdentity {
    Unknown,
    LogicalOnly,
    PhysicallyVerified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub name: &'static str,
    pub version: Option<String>,
    pub selected_config: Option<String>,
    pub read_only: Option<bool>,
    pub protected_manual: bool,
    pub access_controlled: bool,
    pub fan_count: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemperatureReading {
    pub value_c: f64,
    pub role: SensorRole,
    pub trust: Trust,
    pub fresh: bool,
    pub path: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpmReading {
    pub index: u32,
    pub name: String,
    pub value: Option<f64>,
    pub unit: String,
    pub raw_value: Option<String>,
    pub raw_unit: String,
    pub trust: Trust,
    pub fresh: bool,
    pub path: Option<String>,
    pub usable_for_feedback: bool,
    pub verified_rpm: bool,
    /// No verified logical-fan mapping exists unless a future evidence-backed
    /// mapping is added.
    pub logical_fan_index: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlCapability {
    pub state: ControlCapabilityState,
    pub reasons: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl ControlCapability {
    fn available() -> Self {
        Self {
            state: ControlCapabilityState::Available,
            reasons: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn restricted(reasons: Vec<String>) -> Self {
        Self {
            state: ControlCapabilityState::Restricted,
            reasons,
            warnings: Vec::new(),
        }
    }

    fn unavailable(reasons: Vec<String>) -> Self {
        Self {
            state: ControlCapabilityState::Unavailable,
            reasons,
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanInfo {
    pub index: u32,
    pub backend_id: String,
    pub backend_name: &'static str,
    pub display_name: String,
    pub configured_name: String,
    /// Logical role from the backend name. This is not physical identity.
    pub role: FanRole,
    pub physical_identity: PhysicalIdentity,
    pub identity_evidence: Vec<String>,
    pub temperature_sources: Vec<String>,
    pub temperature: Option<TemperatureReading>,
    pub mode: String,
    /// Raw EC mode code, if the read-only EC provider is available.
    pub ec_mode_code: Option<u8>,
    /// Whether NBFC's automatic policy is corroborated by the raw EC mode.
    pub automatic_mode: AutomaticModeVerification,
    pub automatic_mode_evidence: String,
    pub requested_target_percent: Option<f64>,
    pub target_percent: Option<f64>,
    pub reported_percent: Option<f64>,
    /// Logical NBFC fans have no verified Acer RPM mapping. This stays null
    /// until independent evidence maps a channel.
    pub measured_rpm: Option<RpmReading>,
    pub rpm_mapping: &'static str,
    pub manual_control: ControlCapability,
    pub automatic_control: ControlCapability,
    pub regulation_policy_eligible: bool,
    pub regulation_hardware_validated: bool,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanInventory {
    pub schema: &'static str,
    pub generated_at_ms: u64,
    pub supervisor: crate::supervisor::SupervisorState,
    pub backend: BackendInfo,
    pub fans: Vec<FanInfo>,
    pub rpm_channels: Vec<RpmReading>,
    pub cpu_temperature: Option<TemperatureReading>,
    pub gpu_temperature: Option<TemperatureReading>,
    pub notes: Vec<String>,
}

impl FanInventory {
    pub fn fan(&self, index: u32) -> Option<&FanInfo> {
        self.fans.iter().find(|fan| fan.index == index)
    }
}

fn temperature_reading(sample: &SensorSample) -> Option<TemperatureReading> {
    Some(TemperatureReading {
        value_c: sample.value?,
        role: sample.role.clone(),
        trust: sample.health.trust,
        fresh: sample.health.fresh,
        path: sample.path.clone(),
        label: sample.label.clone(),
    })
}

fn source_is_cpu(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("cpu")
        || lower.contains("coretemp")
        || lower.contains("k10temp")
        || lower.contains("zenpower")
}

fn source_is_gpu(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    lower.contains("gpu")
        || lower.contains("amdgpu")
        || lower.contains("nvidia")
        || lower.contains("nouveau")
        || lower.contains("radeon")
}

fn selected_for_sources<'a>(
    snapshot: &'a SensorSnapshot,
    sources: &[String],
    now: u64,
    ttl: u64,
) -> Option<&'a SensorSample> {
    let wants_cpu = sources.iter().any(|source| source_is_cpu(source));
    let wants_gpu = sources.iter().any(|source| source_is_gpu(source));
    // A mixed source list would require reproducing NBFC's averaging policy.
    // Do not choose one side silently.
    if wants_cpu && wants_gpu {
        return None;
    }
    if wants_cpu {
        return snapshot.cpu_package(now, ttl);
    }
    if wants_gpu {
        return snapshot
            .gpu_indexes()
            .into_iter()
            .find_map(|index| snapshot.gpu_die(index, now, ttl));
    }
    None
}

fn capability_for_manual(
    snapshot: &SensorSnapshot,
    status: Option<&NbfcStatus>,
    capabilities: Option<&NbfcCapabilities>,
    sensor_config: Option<&NbfcSensorConfig>,
    fan: &NbfcFan,
    config: &SafetyConfig,
    now: u64,
) -> ControlCapability {
    let (Some(status), Some(capabilities)) = (status, capabilities) else {
        return ControlCapability::unavailable(vec![
            "supervisor does not report NBFC capabilities or status".to_string(),
        ]);
    };
    let floor = config.min_percent as f64;
    let probe = fan
        .target_speed
        .filter(|value| value.is_finite() && (floor..=100.0).contains(value))
        .unwrap_or(50.0);
    let decision = assess_manual_request(
        snapshot,
        status,
        capabilities,
        sensor_config,
        fan.index,
        probe,
        config,
        now,
    );
    if decision.allowed {
        ControlCapability {
            state: ControlCapabilityState::Available,
            reasons: Vec::new(),
            warnings: decision.warnings,
        }
    } else {
        ControlCapability::restricted(decision.reasons)
    }
}

fn automatic_capability(fan: &NbfcFan, status: Option<&NbfcStatus>) -> ControlCapability {
    if status.and_then(|status| status.read_only) == Some(true) {
        return ControlCapability::unavailable(vec![
            "NBFC reports read-only mode; automatic control cannot be written".to_string(),
        ]);
    }
    if status.and_then(|status| status.fan(fan.index)).is_none() {
        return ControlCapability::unavailable(vec![
            "fan disappeared from NBFC status".to_string(),
        ]);
    }
    ControlCapability::available()
}

fn mode_label(fan: &NbfcFan) -> String {
    match fan.auto_mode {
        Some(true) => "automatic".to_string(),
        Some(false) => "manual".to_string(),
        None => "unknown".to_string(),
    }
}

fn rpm_inventory(snapshot: &SensorSnapshot, now: u64, ttl: u64) -> Vec<RpmReading> {
    let mut channels: Vec<RpmReading> = snapshot
        .rpm_sensors()
        .map(|sensor| {
            let index = match sensor.role {
                SensorRole::FanRpm(index) => index,
                _ => 0,
            };
            RpmReading {
                index,
                name: sensor.display_name(),
                value: sensor.value,
                unit: match sensor.unit {
                    crate::model::Unit::Rpm => "rpm".to_string(),
                    crate::model::Unit::Raw => "raw".to_string(),
                    other => format!("{other:?}").to_ascii_lowercase(),
                },
                raw_value: sensor.raw_value.clone(),
                raw_unit: sensor.raw_unit.clone(),
                trust: sensor.health.trust,
                fresh: sensor.value.is_some() && sensor.is_fresh(now, ttl),
                path: sensor.path.clone(),
                usable_for_feedback: sensor.is_usable_for_feedback(now, ttl),
                verified_rpm: sensor.is_usable_for_feedback(now, ttl),
                logical_fan_index: None,
            }
        })
        .collect();
    channels.sort_by_key(|channel| channel.index);
    channels
}

pub fn build(view: &RuntimeView) -> Result<FanInventory, String> {
    let now = now_ms();
    let ttl = SafetyConfig::default().stale_after_ms;
    if !view.supervisor.state.is_connected() {
        return Err(format!(
            "supervisor {}: fan discovery requires the supervisor",
            view.supervisor.state.label()
        ));
    }
    let status = view.status.as_ref();
    let status_doc = status.ok_or_else(|| {
        view.backend_error
            .clone()
            .unwrap_or_else(|| "NBFC status unavailable; fan discovery failed".to_string())
    })?;
    let capabilities = view.version.map(|version| {
        view.capabilities
            .clone()
            .unwrap_or_else(|| NbfcCapabilities::from_version(version))
    });
    let control = control::assess_view(view);
    let config = SafetyConfig::default();
    let cpu = view
        .snapshot
        .cpu_package(now, ttl)
        .and_then(temperature_reading);
    let gpu = view
        .snapshot
        .gpu_indexes()
        .into_iter()
        .find_map(|index| view.snapshot.gpu_die(index, now, ttl))
        .and_then(temperature_reading);
    let rpm_channels = rpm_inventory(&view.snapshot, now, ttl);
    let mut fans = Vec::with_capacity(status_doc.fans.len());
    for fan in &status_doc.fans {
        let readiness = control.fans.iter().find(|item| item.index == fan.index);
        let sources = readiness
            .map(|item| item.sources.clone())
            .unwrap_or_default();
        let source = view
            .sensor_config
            .as_ref()
            .and_then(|config| config.fans.iter().find(|source| source.index == fan.index));
        let mapping = temperature_mapping(
            crate::policy::fan_role(&fan.name),
            source,
            &view.snapshot,
            now,
            ttl,
        );
        let mode_evidence = automatic_mode_evidence(
            crate::policy::fan_role(&fan.name),
            fan.auto_mode,
            &view.snapshot,
            fan.index,
            now,
            ttl,
        );
        let temperature = if mapping.kind == Some(TemperatureMappingKind::IndependentGpuDie) {
            view.snapshot
                .gpu_indexes()
                .into_iter()
                .find_map(|index| view.snapshot.gpu_die(index, now, ttl))
                .and_then(temperature_reading)
        } else if mapping.matched {
            selected_for_sources(&view.snapshot, &sources, now, ttl).and_then(temperature_reading)
        } else {
            None
        };
        let manual = capability_for_manual(
            &view.snapshot,
            status,
            capabilities.as_ref(),
            view.sensor_config.as_ref(),
            fan,
            &config,
            now,
        );
        let automatic = automatic_capability(fan, status);
        let regulation_policy_eligible = readiness.is_some_and(|item| item.policy_eligible);
        let mut blockers = readiness
            .map(|item| item.reasons.clone())
            .unwrap_or_default();
        for reason in &manual.reasons {
            if !blockers.contains(reason) {
                blockers.push(reason.clone());
            }
        }
        let mut warnings = Vec::new();
        if matches!(
            mode_evidence.state,
            AutomaticModeVerification::Unverified | AutomaticModeVerification::Contradicted
        ) {
            warnings.push(mode_evidence.detail.clone());
        }
        if let Some(warning) = &mapping.warning {
            warnings.push(warning.clone());
        }
        warnings.extend(manual.warnings.iter().cloned());
        if sources.is_empty() {
            warnings.push("no NBFC temperature source is declared".to_string());
        } else if temperature.is_none() {
            warnings.push(
                "declared temperature source is unavailable or ambiguous; no value is fabricated"
                    .to_string(),
            );
        }
        if rpm_channels
            .iter()
            .all(|channel| channel.logical_fan_index != Some(fan.index))
        {
            warnings.push(
                "no verified RPM-to-logical-fan mapping; measured RPM is not attached".to_string(),
            );
        }
        if let Some(readiness) = readiness {
            warnings.extend(readiness.validation_unverified.clone());
        }
        let mut seen = std::collections::HashSet::new();
        warnings.retain(|warning| seen.insert(warning.clone()));
        let mut identity_evidence = vec![
            "logical NBFC fan name/index only; no independent physical fan evidence".to_string(),
        ];
        if !rpm_channels.is_empty() {
            identity_evidence.push(
                "provisional EC tachometer candidates are channel-indexed only and do not establish physical identity"
                    .to_string(),
            );
        }
        fans.push(FanInfo {
            index: fan.index,
            backend_id: format!("nbfc:{}", fan.index),
            backend_name: "nbfc",
            display_name: fan.name.clone(),
            configured_name: fan.name.clone(),
            role: crate::policy::fan_role(&fan.name),
            physical_identity: PhysicalIdentity::Unknown,
            identity_evidence,
            temperature_sources: sources,
            temperature,
            mode: mode_label(fan),
            ec_mode_code: mode_evidence.ec_mode,
            automatic_mode: mode_evidence.state,
            automatic_mode_evidence: mode_evidence.detail,
            // NBFC may report a placeholder 0.0 RequestedSpeed in automatic
            // mode; only treat it as a request when the fan is manual.
            requested_target_percent: if fan.auto_mode == Some(false) {
                fan.requested_speed.filter(|value| *value >= 0.0)
            } else {
                None
            },
            target_percent: fan.target_speed,
            reported_percent: fan.current_speed,
            measured_rpm: None,
            rpm_mapping: "unmapped",
            manual_control: manual,
            automatic_control: automatic,
            regulation_policy_eligible,
            regulation_hardware_validated: control.physical_control_validated,
            blockers,
            warnings,
        });
    }
    let mut notes = vec![
        "Logical fan indexes and names come from the backend; they are not physical fan identities."
            .to_string(),
        "Measured RPM is listed globally only when no verified logical-fan mapping exists."
            .to_string(),
    ];
    if rpm_channels.is_empty() {
        notes.push("No independent RPM provider is available; RPM values are N/A.".to_string());
    } else if rpm_channels
        .iter()
        .all(|channel| channel.trust == Trust::Provisional)
    {
        notes.push(
            "Provisional EC tachometer candidates are present but are not validated RPM feedback and are not mapped to logical fans."
                .to_string(),
        );
    }
    Ok(FanInventory {
        schema: SCHEMA,
        generated_at_ms: now,
        supervisor: view.supervisor.state,
        backend: BackendInfo {
            name: "nbfc",
            version: view.version.map(|version| version.to_string()),
            selected_config: status_doc.selected_config.clone(),
            read_only: status_doc.read_only,
            protected_manual: capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.protected_manual),
            access_controlled: capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.access_controlled),
            fan_count: status_doc.fans.len(),
            error: view.backend_error.clone(),
        },
        fans,
        rpm_channels,
        cpu_temperature: cpu,
        gpu_temperature: gpu,
        notes,
    })
}

/// Stable per-fan report wrapper for `fans show --json`.
#[derive(Debug, Clone, Serialize)]
pub struct FanReport<'a> {
    pub schema: &'static str,
    #[serde(flatten)]
    pub inventory: &'a FanInventory,
    pub fan: &'a FanInfo,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SensorKind, SensorProvider, SensorSample, Unit};
    use crate::nbfc::{NbfcFan, NbfcSensorConfig};
    use crate::runtime::RuntimeView;
    use crate::service::ServiceStatus;
    use crate::supervisor::{SupervisorProbe, SupervisorState};

    fn temp(role: SensorRole, value: f64) -> SensorSample {
        SensorSample::new(
            format!("test:{role}"),
            role,
            SensorKind::Temperature,
            Some(value),
            Unit::Celsius,
            Some(value.to_string()),
            "celsius",
            SensorProvider::Hwmon,
            "test".into(),
            Some("/sys/test".into()),
            None,
            None,
        )
    }

    fn view(with_gpu: bool, with_rpm: bool) -> RuntimeView {
        let version = crate::nbfc::NbfcVersion::parse("nbfc 0.5.4").unwrap();
        let mut capabilities = crate::nbfc::NbfcCapabilities::from_version(version);
        capabilities.access_controlled = true;
        let mut sensors = vec![temp(SensorRole::CpuPackage, 60.0)];
        if with_gpu {
            sensors.push(temp(SensorRole::GpuDie(0), 55.0));
        }
        if with_rpm {
            sensors.push(SensorSample::new(
                "test:rpm".into(),
                SensorRole::FanRpm(0),
                SensorKind::Rpm,
                Some(4000.0),
                Unit::Rpm,
                Some("4000".into()),
                "rpm",
                SensorProvider::Hwmon,
                "acer".into(),
                Some("/sys/acer/fan1_input".into()),
                None,
                None,
            ));
        }
        RuntimeView {
            snapshot: SensorSnapshot {
                sensors,
                errors: Vec::new(),
                collected_at_ms: now_ms(),
            },
            sensor_source_error: None,
            version: Some(version),
            capabilities: Some(capabilities),
            status: Some(NbfcStatus {
                version: None,
                read_only: Some(false),
                selected_config: Some("test".into()),
                fans: vec![
                    NbfcFan {
                        index: 0,
                        name: "CPU fan".into(),
                        temperature: Some(60.0),
                        critical_temperature: Some(95.0),
                        auto_mode: Some(true),
                        critical: Some(false),
                        current_speed: Some(40.0),
                        target_speed: Some(40.0),
                        requested_speed: Some(-1.0),
                        speed_steps: Some(100),
                    },
                    NbfcFan {
                        index: 1,
                        name: "GPU fan".into(),
                        temperature: Some(60.0),
                        critical_temperature: Some(95.0),
                        auto_mode: Some(true),
                        critical: Some(false),
                        current_speed: Some(50.0),
                        target_speed: Some(50.0),
                        requested_speed: Some(-1.0),
                        speed_steps: Some(100),
                    },
                ],
                warnings: Vec::new(),
            }),
            service: ServiceStatus::default(),
            sensor_config: Some(NbfcSensorConfig {
                fans: vec![
                    crate::nbfc::NbfcFanSensor {
                        index: 0,
                        name: "CPU fan".into(),
                        sensors: vec!["@CPU".into()],
                        algorithm: Some("Average".into()),
                    },
                    crate::nbfc::NbfcFanSensor {
                        index: 1,
                        name: "GPU fan".into(),
                        sensors: vec!["@CPU".into()],
                        algorithm: Some("Average".into()),
                    },
                ],
            }),
            backend_error: None,
            supervisor: SupervisorProbe {
                state: SupervisorState::Healthy,
                socket: "/run/neosense/neosensed.sock".into(),
                socket_exists: true,
                socket_mode: Some("660".into()),
                socket_uid: Some(0),
                socket_gid: Some(0),
                health: Some(true),
                reason: None,
                error: None,
                status: None,
            },
        }
    }

    #[test]
    fn inventory_lists_every_logical_fan_and_keeps_identity_unknown() {
        let inventory = build(&view(true, true)).unwrap();
        assert_eq!(inventory.schema, SCHEMA);
        assert_eq!(inventory.fans.len(), 2);
        assert_eq!(inventory.fans[0].backend_id, "nbfc:0");
        assert_eq!(inventory.fans[0].role, FanRole::Cpu);
        assert_eq!(
            inventory.fans[0].physical_identity,
            PhysicalIdentity::Unknown
        );
        assert_eq!(inventory.fans[0].rpm_mapping, "unmapped");
        assert_eq!(inventory.fans[0].requested_target_percent, None);
        assert!(inventory.fans[0].measured_rpm.is_none());
        assert_eq!(inventory.rpm_channels.len(), 1);
        assert_eq!(inventory.rpm_channels[0].logical_fan_index, None);
    }

    #[test]
    fn provisional_ec_tach_channel_is_raw_and_not_verified_rpm() {
        let now = crate::model::now_ms();
        let snapshot = SensorSnapshot {
            sensors: vec![SensorSample::new(
                "acer_ec:fan0.rpm".into(),
                SensorRole::FanRpm(0),
                SensorKind::Rpm,
                Some(4225.0),
                Unit::Raw,
                Some("0x81:0x10".into()),
                "ec_u16_provisional",
                SensorProvider::Ec,
                "acer_ec".into(),
                Some("/sys/kernel/debug/ec/ec0/io".into()),
                None,
                Some("acer-ec:ec0".into()),
            )],
            errors: Vec::new(),
            collected_at_ms: now,
        };
        let channels = rpm_inventory(&snapshot, now, 3_000);
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].unit, "raw");
        assert_eq!(channels[0].raw_unit, "ec_u16_provisional");
        assert_eq!(channels[0].raw_value.as_deref(), Some("0x81:0x10"));
        assert_eq!(channels[0].trust, Trust::Provisional);
        assert!(!channels[0].usable_for_feedback);
        assert!(!channels[0].verified_rpm);
    }

    #[test]
    fn gpu_fan_uses_independent_temperature_but_keeps_identity_unknown() {
        let inventory = build(&view(true, true)).unwrap();
        let gpu = inventory.fan(1).unwrap();
        assert_eq!(gpu.temperature_sources, vec!["@CPU"]);
        assert!(gpu.temperature.is_some());
        assert_eq!(
            gpu.temperature.as_ref().unwrap().role,
            SensorRole::GpuDie(0)
        );
        assert_eq!(gpu.manual_control.state, ControlCapabilityState::Available);
        assert_eq!(gpu.physical_identity, PhysicalIdentity::Unknown);
        assert_eq!(
            gpu.warnings
                .iter()
                .filter(|warning| warning.contains("independent NVIDIA GPU temperature"))
                .count(),
            1
        );
    }

    #[test]
    fn gpu_fan_stays_blocked_without_independent_temperature() {
        let inventory = build(&view(false, true)).unwrap();
        let gpu = inventory.fan(1).unwrap();
        assert_eq!(gpu.temperature_sources, vec!["@CPU"]);
        assert!(gpu.temperature.is_none());
        assert_eq!(gpu.manual_control.state, ControlCapabilityState::Restricted);
        assert_eq!(gpu.physical_identity, PhysicalIdentity::Unknown);
    }

    #[test]
    fn manual_mode_exposes_requested_target_while_auto_does_not() {
        let mut view = view(true, true);
        let fan = &mut view.status.as_mut().unwrap().fans[0];
        fan.auto_mode = Some(false);
        fan.requested_speed = Some(40.0);
        let inventory = build(&view).unwrap();
        assert_eq!(
            inventory.fan(0).unwrap().requested_target_percent,
            Some(40.0)
        );
        assert_eq!(inventory.fan(0).unwrap().mode, "manual");
    }

    #[test]
    fn manual_control_can_be_available_without_regulation_validation() {
        let inventory = build(&view(true, true)).unwrap();
        let cpu = inventory.fan(0).unwrap();
        assert_eq!(cpu.manual_control.state, ControlCapabilityState::Available);
        assert!(!cpu.regulation_hardware_validated);
        assert_eq!(cpu.physical_identity, PhysicalIdentity::Unknown);
        assert_eq!(
            cpu.automatic_control.state,
            ControlCapabilityState::Available
        );
    }
}
