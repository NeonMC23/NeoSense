//! Thermal safety policy.
//!
//! The policy is pure: it receives a sensor snapshot and backend metadata and
//! returns a decision. The daemon is responsible for executing (or refusing)
//! writes and for verifying the result. Tests exercise the policy without any
//! hardware access.

use crate::model::{SensorSnapshot, SensorStatus, now_ms};
use crate::nbfc::{NbfcCapabilities, NbfcSensorConfig, NbfcStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConfig {
    /// A sensor sample older than this is stale for control decisions.
    pub stale_after_ms: u64,
    /// Manual control automatically expires after this delay.
    pub manual_ttl_ms: u64,
    /// Lowest duty cycle accepted in manual mode. Zero is never allowed.
    pub min_percent: u8,
    /// Manual control is refused above this temperature.
    pub max_temp_c: f64,
    /// Refuse GPU manual control unless the NBFC fan is actually driven by a
    /// GPU sensor group.
    pub require_gpu_sensor_source: bool,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            stale_after_ms: 3_000,
            manual_ttl_ms: 15 * 60 * 1_000,
            min_percent: 20,
            max_temp_c: 85.0,
            require_gpu_sensor_source: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanRole {
    Cpu,
    Gpu,
    Unknown,
}

impl std::fmt::Display for FanRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FanRole::Cpu => write!(f, "CPU"),
            FanRole::Gpu => write!(f, "GPU"),
            FanRole::Unknown => write!(f, "unknown"),
        }
    }
}

pub fn fan_role(name: &str) -> FanRole {
    let lower = name.to_ascii_lowercase();
    if lower.contains("cpu") {
        FanRole::Cpu
    } else if lower.contains("gpu") {
        FanRole::Gpu
    } else {
        FanRole::Unknown
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    pub fan_role: FanRole,
    pub max_temperature_c: Option<f64>,
    pub min_percent: u8,
    pub ttl_ms: u64,
}

fn source_mentions_cpu(source: &crate::nbfc::NbfcFanSensor) -> bool {
    source.sensors.iter().any(|s| {
        let lower = s.to_ascii_lowercase();
        lower.contains("cpu")
            || lower.contains("coretemp")
            || lower.contains("k10temp")
            || lower.contains("zenpower")
    })
}

fn source_mentions_gpu(source: &crate::nbfc::NbfcFanSensor) -> bool {
    source.sensors.iter().any(|s| {
        let lower = s.to_ascii_lowercase();
        lower.contains("gpu")
            || lower.contains("amdgpu")
            || lower.contains("nvidia")
            || lower.contains("nouveau")
            || lower.contains("radeon")
    })
}

impl PolicyDecision {
    fn deny(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reasons: vec![reason.into()],
            warnings: Vec::new(),
            fan_role: FanRole::Unknown,
            max_temperature_c: None,
            min_percent: 0,
            ttl_ms: 0,
        }
    }
}

/// Assess a manual fan request. A `false` result is a safety refusal, not a
/// backend failure; the CLI maps it to exit code 8.
#[allow(clippy::too_many_arguments)]
pub fn assess_manual_request(
    snapshot: &SensorSnapshot,
    status: &NbfcStatus,
    capabilities: &NbfcCapabilities,
    sensor_config: Option<&NbfcSensorConfig>,
    fan_index: u32,
    percent: f64,
    config: &SafetyConfig,
    now: u64,
) -> PolicyDecision {
    let Some(fan) = status.fan(fan_index) else {
        return PolicyDecision::deny(format!("ventilateur {fan_index} introuvable"));
    };
    let role = fan_role(&fan.name);
    let mut reasons = Vec::new();
    let mut warnings = Vec::new();

    if !capabilities.protected_manual {
        reasons.push(format!(
            "NBFC {} does not provide the required guarantees (>= 0.5.4 is required for protected manual control)",
            capabilities.version
        ));
    }
    if status.read_only == Some(true) {
        reasons.push("the NBFC service is read-only".to_string());
    }
    if status.any_critical() {
        reasons.push("NBFC is in critical mode; manual control is refused".to_string());
    }
    if role == FanRole::Unknown {
        reasons.push(format!(
            "fan role {:?} is ambiguous; no manual write allowed",
            fan.name
        ));
    }
    if !(config.min_percent as f64..=100.0).contains(&percent) {
        reasons.push(format!(
            "speed {percent:.1}% is outside the allowed range {}–100%",
            config.min_percent
        ));
    }
    if now.saturating_sub(snapshot.collected_at_ms) > config.stale_after_ms {
        reasons.push(format!(
            "sensor snapshot is stale ({} ms)",
            now.saturating_sub(snapshot.collected_at_ms)
        ));
    }

    let cpu = snapshot.cpu_package(now, config.stale_after_ms);
    if cpu.is_none() {
        reasons.push("no valid, fresh and trustworthy CPU temperature".to_string());
    }

    let gpu = snapshot
        .gpu_indexes()
        .into_iter()
        .next()
        .and_then(|index| snapshot.gpu_die(index, now, config.stale_after_ms));
    let source = sensor_config.and_then(|cfg| cfg.fans.iter().find(|f| f.index == fan_index));
    if source.is_none() {
        reasons.push(
            "NBFC sensor source is unknown; manual control requires verifiable provenance"
                .to_string(),
        );
    }
    if role == FanRole::Cpu
        && let Some(source) = source
        && !source_mentions_cpu(source)
    {
        reasons.push(format!(
            "CPU fan uses {:?} as its source; an explicit CPU source is required",
            source.sensors
        ));
    }
    if role == FanRole::Gpu {
        if gpu.is_none() {
            reasons.push("no valid, fresh and trustworthy GPU temperature".to_string());
        }
        if config.require_gpu_sensor_source {
            match source {
                Some(source) if source_mentions_gpu(source) => {}
                Some(source) => reasons.push(format!(
                    "GPU fan uses {:?} as its source; an explicit GPU source is required",
                    source.sensors
                )),
                None => reasons
                    .push("GPU fan sensor source is unknown; manual control refused".to_string()),
            }
        }
    }

    let max_temperature_c = [cpu.and_then(|s| s.value), gpu.and_then(|s| s.value)]
        .into_iter()
        .flatten()
        .reduce(f64::max);
    if let Some(temp) = max_temperature_c
        && temp > config.max_temp_c
    {
        reasons.push(format!(
            "temperature {temp:.1} °C exceeds the manual-control ceiling {:.1} °C",
            config.max_temp_c
        ));
    }

    // Missing data must never look like a safe temperature.
    if let Some(cpu) = cpu
        && cpu.status != SensorStatus::Ok
    {
        reasons.push("CPU sensor is invalid".to_string());
    }
    if role == FanRole::Gpu
        && let Some(gpu) = gpu
        && gpu.status != SensorStatus::Ok
    {
        reasons.push("GPU sensor is invalid".to_string());
    }
    if let Some(cpu) = cpu
        && !cpu.health.trust.is_at_least(crate::model::Trust::Trusted)
    {
        warnings.push("CPU sensor has limited trust".to_string());
    }

    PolicyDecision {
        allowed: reasons.is_empty(),
        reasons,
        warnings,
        fan_role: role,
        max_temperature_c,
        min_percent: config.min_percent,
        ttl_ms: config.manual_ttl_ms,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyState {
    SafeAuto,
    Manual,
    Degraded,
    FailSafe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualLease {
    pub fan_index: u32,
    pub percent: f64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyReport {
    pub state: SafetyState,
    pub reasons: Vec<String>,
    pub manual: Option<ManualLease>,
    pub updated_at_ms: u64,
}

impl SafetyReport {
    pub fn safe_auto() -> Self {
        Self {
            state: SafetyState::SafeAuto,
            reasons: Vec::new(),
            manual: None,
            updated_at_ms: now_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SensorKind, SensorProvider, SensorRole, SensorSample, Trust, Unit};

    fn temp(role: SensorRole, value: f64, trust: Trust) -> SensorSample {
        SensorSample::new(
            format!("test:{role}"),
            role,
            SensorKind::Temperature,
            Some(value),
            Unit::Celsius,
            Some(value.to_string()),
            "celsius",
            SensorProvider::Hwmon,
            "test".to_string(),
            None,
            None,
            None,
        )
        .with_trust(trust)
    }

    fn snapshot(cpu: Option<f64>, gpu: Option<f64>) -> SensorSnapshot {
        let mut sensors = Vec::new();
        if let Some(v) = cpu {
            sensors.push(temp(SensorRole::CpuPackage, v, Trust::Verified));
        }
        if let Some(v) = gpu {
            sensors.push(temp(SensorRole::GpuDie(0), v, Trust::Verified));
        }
        SensorSnapshot {
            sensors,
            errors: Vec::new(),
            collected_at_ms: now_ms(),
        }
    }

    fn status(name: &str, auto: bool) -> NbfcStatus {
        NbfcStatus {
            version: None,
            read_only: Some(false),
            selected_config: Some("test".into()),
            fans: vec![crate::nbfc::NbfcFan {
                index: 0,
                name: name.into(),
                temperature: Some(55.0),
                critical_temperature: Some(95.0),
                auto_mode: Some(auto),
                critical: Some(false),
                current_speed: Some(50.0),
                target_speed: Some(50.0),
                requested_speed: Some(50.0),
                speed_steps: Some(100),
            }],
            warnings: Vec::new(),
        }
    }

    fn caps(version: &str) -> NbfcCapabilities {
        NbfcCapabilities::from_version(crate::nbfc::NbfcVersion::parse(version).unwrap())
    }

    fn sensors(_cpu: bool, gpu: bool) -> NbfcSensorConfig {
        NbfcSensorConfig {
            fans: vec![crate::nbfc::NbfcFanSensor {
                index: 0,
                name: "CPU fan".into(),
                sensors: vec![if gpu { "@GPU" } else { "@CPU" }.into()],
                algorithm: Some("Average".into()),
            }],
        }
    }

    #[test]
    fn refuses_manual_on_nbfc_053() {
        let decision = assess_manual_request(
            &snapshot(Some(55.0), Some(55.0)),
            &status("CPU fan", false),
            &caps("nbfc 0.5.3"),
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(!decision.allowed);
        assert!(decision.reasons.iter().any(|r| r.contains("0.5.4")));
    }

    #[test]
    fn allows_cpu_manual_when_all_evidence_is_present() {
        let decision = assess_manual_request(
            &snapshot(Some(55.0), Some(55.0)),
            &status("CPU fan", false),
            &caps("nbfc 0.5.4"),
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(decision.allowed, "{:?}", decision.reasons);
    }

    #[test]
    fn refuses_gpu_manual_when_fan_uses_cpu_sensor() {
        let decision = assess_manual_request(
            &snapshot(Some(55.0), Some(80.0)),
            &status("GPU fan", false),
            &caps("nbfc 0.5.4"),
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(!decision.allowed);
        assert!(decision.reasons.iter().any(|r| r.contains("source")));
    }

    #[test]
    fn refuses_zero_and_below_floor() {
        for percent in [0.0, 19.0] {
            let decision = assess_manual_request(
                &snapshot(Some(55.0), Some(55.0)),
                &status("CPU fan", false),
                &caps("nbfc 0.5.4"),
                Some(&sensors(true, false)),
                0,
                percent,
                &SafetyConfig::default(),
                now_ms(),
            );
            assert!(!decision.allowed, "percent {percent} should be refused");
        }
    }

    #[test]
    fn refuses_when_temperature_is_too_high_or_missing() {
        let hot = assess_manual_request(
            &snapshot(Some(90.0), Some(60.0)),
            &status("CPU fan", false),
            &caps("nbfc 0.5.4"),
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(!hot.allowed);
        let missing = assess_manual_request(
            &snapshot(None, Some(60.0)),
            &status("CPU fan", false),
            &caps("nbfc 0.5.4"),
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(!missing.allowed);
    }

    #[test]
    fn refuses_stale_snapshot() {
        let mut snap = snapshot(Some(55.0), Some(55.0));
        snap.collected_at_ms = 1;
        let decision = assess_manual_request(
            &snap,
            &status("CPU fan", false),
            &caps("nbfc 0.5.4"),
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(!decision.allowed);
    }
}
