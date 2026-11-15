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

/// How a logical fan's temperature guard is bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureMappingKind {
    /// The NBFC fan sensor source names the fan's own role.
    NbfcSource,
    /// The NBFC source does not name a GPU, but a fresh, trusted NVIDIA GPU
    /// die reading is available as an independent guard.
    IndependentGpuDie,
}

/// Evidence for the temperature source used by one logical fan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemperatureMapping {
    pub matched: bool,
    pub kind: Option<TemperatureMappingKind>,
    pub source: Option<String>,
    pub reason: Option<String>,
    pub warning: Option<String>,
}

impl TemperatureMapping {
    fn missing(reason: impl Into<String>) -> Self {
        Self {
            matched: false,
            kind: None,
            source: None,
            reason: Some(reason.into()),
            warning: None,
        }
    }
}

/// Resolve the temperature guard for a logical fan.
///
/// CPU fans remain strict: their NBFC source must name a CPU. GPU fans may use
/// a fresh, trusted NVIDIA GPU die reading when NBFC is configured with a
/// non-GPU source such as `@CPU`. That fallback is explicit, is never inferred
/// from CPU temperature, and does not establish physical fan identity.
pub fn temperature_mapping(
    role: FanRole,
    source: Option<&crate::nbfc::NbfcFanSensor>,
    snapshot: &SensorSnapshot,
    now: u64,
    ttl_ms: u64,
) -> TemperatureMapping {
    let declared = source
        .map(|source| source.sensors.join(","))
        .unwrap_or_else(|| "unknown".to_string());
    if role == FanRole::Unknown {
        return TemperatureMapping::missing("fan role is ambiguous");
    }
    let Some(source) = source else {
        return TemperatureMapping::missing(format!(
            "fan role {role} has no NBFC sensor source; verifiable provenance is required"
        ));
    };
    let source_matches = match role {
        FanRole::Cpu => source_mentions_cpu(source),
        FanRole::Gpu => source_mentions_gpu(source),
        FanRole::Unknown => false,
    };
    if source_matches {
        return TemperatureMapping {
            matched: true,
            kind: Some(TemperatureMappingKind::NbfcSource),
            source: Some(declared),
            reason: None,
            warning: None,
        };
    }

    if role == FanRole::Gpu
        && let Some(gpu) = snapshot
            .gpu_indexes()
            .into_iter()
            .find_map(|index| snapshot.gpu_die(index, now, ttl_ms))
    {
        return TemperatureMapping {
            matched: true,
            kind: Some(TemperatureMappingKind::IndependentGpuDie),
            source: Some(gpu.id.clone()),
            reason: None,
            warning: Some(format!(
                "NBFC source [{declared}] does not name a GPU; using independent NVIDIA GPU temperature {} as the guard; physical fan identity remains unverified",
                gpu.id
            )),
        };
    }

    TemperatureMapping::missing(format!(
        "fan sources [{declared}] do not match role {role}; no independent GPU temperature is available"
    ))
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

/// Whether NBFC's claimed automatic policy is corroborated by a raw EC mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticModeVerification {
    /// NBFC reports automatic and the EC mode code matches automatic.
    Verified,
    /// NBFC reports automatic, but the EC mode is absent, manual or unknown.
    Unverified,
    /// NBFC and EC mode evidence disagree about automatic/manual ownership.
    Contradicted,
    /// NBFC reports manual control.
    BackendManual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomaticModeEvidence {
    pub state: AutomaticModeVerification,
    pub backend_auto: Option<bool>,
    pub ec_mode: Option<u8>,
    pub detail: String,
}

/// Interpret the raw EC fan-mode code against NBFC's reported mode.
///
/// NBFC's `AutoMode=true` is its internal software policy, not proof that the
/// EC/firmware is in automatic mode. Register values are provisional evidence
/// from the community `acer-nitro-ec` mapping.
pub fn automatic_mode_evidence(
    role: FanRole,
    backend_auto: Option<bool>,
    snapshot: &SensorSnapshot,
    fan_index: u32,
    now: u64,
    ttl_ms: u64,
) -> AutomaticModeEvidence {
    let ec_mode = snapshot.fan_mode_code(fan_index, now, ttl_ms);
    let (auto_code, manual_code) = match role {
        FanRole::Cpu => (Some(0x04), Some(0x0c)),
        FanRole::Gpu => (Some(0x10), Some(0x30)),
        FanRole::Unknown => (None, None),
    };
    let automatic = ec_mode.is_some() && ec_mode == auto_code;
    let manual = ec_mode.is_some() && ec_mode == manual_code;
    let (state, detail) = match backend_auto {
        None => (
            AutomaticModeVerification::Unverified,
            "NBFC did not report an automatic state".to_string(),
        ),
        Some(false) if automatic => (
            AutomaticModeVerification::Contradicted,
            format!(
                "NBFC reports manual control, but EC mode 0x{:02x} is automatic",
                ec_mode.unwrap_or_default()
            ),
        ),
        Some(false) => (
            AutomaticModeVerification::BackendManual,
            match ec_mode {
                Some(code) if manual => {
                    format!("NBFC reports manual control and EC mode 0x{code:02x} is manual")
                }
                Some(code) => format!(
                    "NBFC reports manual control; EC mode 0x{code:02x} is not automatic/manual as expected"
                ),
                None => "NBFC reports manual control; no EC mode reading is available".to_string(),
            },
        ),
        Some(true) if automatic => (
            AutomaticModeVerification::Verified,
            format!(
                "NBFC reports automatic control and EC mode 0x{:02x} is automatic",
                ec_mode.unwrap_or_default()
            ),
        ),
        Some(true) => (
            AutomaticModeVerification::Unverified,
            match ec_mode {
                Some(code) if manual => format!(
                    "NBFC reports automatic control, but EC mode 0x{code:02x} is manual; NBFC software automatic policy owns a manual EC mode"
                ),
                Some(code) => format!(
                    "NBFC reports automatic control, but EC mode 0x{code:02x} is not recognized as automatic"
                ),
                None => {
                    "NBFC reports automatic control; no EC mode reading is available".to_string()
                }
            },
        ),
    };
    AutomaticModeEvidence {
        state,
        backend_auto,
        ec_mode,
        detail,
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
        return PolicyDecision::deny(format!("fan {fan_index} not found"));
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
    if !capabilities.access_controlled {
        reasons.push(
            "NBFC socket is not access-controlled; an untrusted local client could bypass the NeoSense supervisor"
                .to_string(),
        );
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
    let mapping = temperature_mapping(role, source, snapshot, now, config.stale_after_ms);
    if role == FanRole::Cpu && !mapping.matched {
        reasons.push(
            mapping
                .reason
                .clone()
                .unwrap_or_else(|| "CPU temperature mapping is unavailable".to_string()),
        );
    }
    if role == FanRole::Gpu {
        if gpu.is_none() {
            reasons.push("no valid, fresh and trustworthy GPU temperature".to_string());
        }
        if config.require_gpu_sensor_source && !mapping.matched {
            reasons.push(
                mapping
                    .reason
                    .clone()
                    .unwrap_or_else(|| "GPU temperature mapping is unavailable".to_string()),
            );
        }
        if let Some(warning) = mapping.warning.clone() {
            warnings.push(warning);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyState {
    SafeAuto,
    Manual,
    /// Supervisor-owned, bounded physical-validation sequence.
    Validation,
    /// Supervisor-owned closed-loop profile regulation.
    Profile,
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
    /// Multi-fan fixed preset leases. Never present at the same time as
    /// `manual`; both are cleared by automatic recovery.
    #[serde(default)]
    pub preset: Option<Vec<ManualLease>>,
    pub updated_at_ms: u64,
}

impl SafetyReport {
    pub fn safe_auto() -> Self {
        Self {
            state: SafetyState::SafeAuto,
            reasons: Vec::new(),
            manual: None,
            preset: None,
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
        let mut capabilities =
            NbfcCapabilities::from_version(crate::nbfc::NbfcVersion::parse(version).unwrap());
        // Unit tests model a backend whose socket is already access-controlled;
        // a dedicated test exercises the insecure boundary.
        capabilities.access_controlled = true;
        capabilities
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
    fn refuses_manual_when_nbfc_socket_is_not_access_controlled() {
        let mut insecure = caps("nbfc 0.5.4");
        insecure.access_controlled = false;
        let decision = assess_manual_request(
            &snapshot(Some(55.0), Some(55.0)),
            &status("CPU fan", false),
            &insecure,
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(!decision.allowed);
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.contains("access-controlled"))
        );
    }

    #[test]
    fn gpu_manual_uses_independent_gpu_temperature_when_nbfc_source_is_cpu() {
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
        assert!(decision.allowed, "{:?}", decision.reasons);
        assert!(
            decision
                .warnings
                .iter()
                .any(|warning| warning.contains("independent NVIDIA GPU temperature"))
                && decision
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("physical fan identity remains unverified"))
        );
        assert_eq!(decision.max_temperature_c, Some(80.0));
    }

    #[test]
    fn gpu_manual_stays_blocked_when_independent_temperature_is_stale() {
        let mut snap = snapshot(Some(55.0), Some(80.0));
        snap.sensors[1].sampled_at_ms = now_ms().saturating_sub(10_000);
        let decision = assess_manual_request(
            &snap,
            &status("GPU fan", false),
            &caps("nbfc 0.5.4"),
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(!decision.allowed);
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.contains("GPU temperature"))
        );
    }

    #[test]
    fn gpu_manual_stays_blocked_without_independent_gpu_temperature() {
        let decision = assess_manual_request(
            &snapshot(Some(55.0), None),
            &status("GPU fan", false),
            &caps("nbfc 0.5.4"),
            Some(&sensors(true, false)),
            0,
            50.0,
            &SafetyConfig::default(),
            now_ms(),
        );
        assert!(!decision.allowed);
        assert!(
            decision
                .reasons
                .iter()
                .any(|reason| reason.contains("GPU") && reason.contains("temperature"))
        );
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

    fn mode_snapshot(index: u32, code: u8) -> SensorSnapshot {
        let mut sample = SensorSample::new(
            format!("test:fan{index}.mode"),
            SensorRole::FanMode(index),
            SensorKind::Mode,
            Some(f64::from(code)),
            Unit::Code,
            Some(format!("0x{code:02x}")),
            "code",
            SensorProvider::Ec,
            "test".to_string(),
            None,
            None,
            None,
        );
        sample.health.trust = Trust::Provisional;
        sample.status = crate::model::SensorStatus::Ok;
        SensorSnapshot {
            sensors: vec![sample],
            errors: Vec::new(),
            collected_at_ms: now_ms(),
        }
    }

    #[test]
    fn automatic_mode_is_verified_only_when_ec_code_matches() {
        let now = now_ms();
        let verified = automatic_mode_evidence(
            FanRole::Cpu,
            Some(true),
            &mode_snapshot(0, 0x04),
            0,
            now,
            5_000,
        );
        assert_eq!(verified.state, AutomaticModeVerification::Verified);
        assert_eq!(verified.ec_mode, Some(0x04));

        let gpu = automatic_mode_evidence(
            FanRole::Gpu,
            Some(true),
            &mode_snapshot(1, 0x10),
            1,
            now,
            5_000,
        );
        assert_eq!(gpu.state, AutomaticModeVerification::Verified);
    }

    #[test]
    fn nbfc_auto_with_manual_ec_mode_is_unverified_not_verified() {
        let now = now_ms();
        let evidence = automatic_mode_evidence(
            FanRole::Cpu,
            Some(true),
            &mode_snapshot(0, 0x0c),
            0,
            now,
            5_000,
        );
        assert_eq!(evidence.state, AutomaticModeVerification::Unverified);
        assert!(evidence.detail.contains("manual EC mode"));

        let missing = automatic_mode_evidence(
            FanRole::Cpu,
            Some(true),
            &SensorSnapshot::empty(),
            0,
            now,
            5_000,
        );
        assert_eq!(missing.state, AutomaticModeVerification::Unverified);
        assert_eq!(missing.ec_mode, None);
    }

    #[test]
    fn backend_manual_with_automatic_ec_mode_is_contradicted() {
        let now = now_ms();
        let evidence = automatic_mode_evidence(
            FanRole::Cpu,
            Some(false),
            &mode_snapshot(0, 0x04),
            0,
            now,
            5_000,
        );
        assert_eq!(evidence.state, AutomaticModeVerification::Contradicted);

        let manual = automatic_mode_evidence(
            FanRole::Gpu,
            Some(false),
            &mode_snapshot(1, 0x30),
            1,
            now,
            5_000,
        );
        assert_eq!(manual.state, AutomaticModeVerification::BackendManual);
    }

    #[test]
    fn unknown_role_cannot_claim_automatic_verification() {
        let now = now_ms();
        let evidence = automatic_mode_evidence(
            FanRole::Unknown,
            Some(true),
            &mode_snapshot(0, 0x04),
            0,
            now,
            5_000,
        );
        assert_eq!(evidence.state, AutomaticModeVerification::Unverified);
    }
}
