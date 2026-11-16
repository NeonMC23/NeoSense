//! Read-only calibration-readiness gate.
//!
//! This module answers whether the evidence needed for a future, separately
//! authorized physical calibration exists. It never starts a test, writes a
//! fan target, changes a mode or treats backend acceptance as physical
//! evidence. Missing or contradictory evidence keeps the result blocked.
//!
//! The evaluator deliberately requires an independently verified physical fan
//! identity, an independent RPM/reference channel mapped to a logical fan,
//! validated actuator semantics and verified automatic recovery. On this
//! machine those validation requirements are not yet recorded, so the result
//! remains blocked even when the backend and temperatures look healthy.

use crate::control::{self, ControlReadiness, RequirementStatus};
use crate::fans::PhysicalIdentity;
use crate::model::{SensorSnapshot, Trust, Unit, now_ms};
use crate::nbfc::NbfcStatus;
use crate::policy::{FanRole, SafetyConfig, fan_role};
use crate::runtime::RuntimeView;
use crate::validation::{
    CPU_MAX_TEMP_C, EXPERIMENTAL_MIN_PERCENT, GPU_MAX_TEMP_C, ValidationLimits, ValidationStep,
};
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "neosense.calibration_readiness/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationState {
    /// At least one essential prerequisite is missing or failed.
    #[default]
    Blocked,
    /// Essential prerequisites are present, but validation evidence is still
    /// unverified or incomplete.
    PartiallyReady,
    /// All information prerequisites are present. A separate explicit
    /// authorization is still mandatory; this state never authorizes a test.
    ReadyForSeparatelyAuthorizedTest,
}

impl CalibrationState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::PartiallyReady => "partially ready",
            Self::ReadyForSeparatelyAuthorizedTest => "ready for separately authorized test",
        }
    }

    pub fn is_ready(self) -> bool {
        self == Self::ReadyForSeparatelyAuthorizedTest
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationRequirementId {
    RuntimeBackend,
    LogicalChannel,
    PhysicalIdentity,
    IndependentRpm,
    CpuTelemetry,
    GpuTelemetry,
    ActuatorSemantics,
    RecoveryPath,
    TemperatureCeilings,
    ExperimentalFloor,
}

impl CalibrationRequirementId {
    pub fn label(self) -> &'static str {
        match self {
            Self::RuntimeBackend => "Runtime backend verified",
            Self::LogicalChannel => "Logical channel identified",
            Self::PhysicalIdentity => "Physical fan identity",
            Self::IndependentRpm => "Independent RPM reference",
            Self::CpuTelemetry => "Trusted CPU telemetry",
            Self::GpuTelemetry => "Trusted GPU telemetry",
            Self::ActuatorSemantics => "Validated actuator semantics",
            Self::RecoveryPath => "Recovery path verified",
            Self::TemperatureCeilings => "Temperature ceilings valid",
            Self::ExperimentalFloor => "Experimental floor enforced",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationRequirement {
    pub id: CalibrationRequirementId,
    pub label: String,
    pub status: RequirementStatus,
    pub detail: String,
    pub evidence: String,
}

impl CalibrationRequirement {
    fn new(
        id: CalibrationRequirementId,
        status: RequirementStatus,
        detail: impl Into<String>,
        evidence: impl Into<String>,
    ) -> Self {
        Self {
            id,
            label: id.label().to_string(),
            status,
            detail: detail.into(),
            evidence: evidence.into(),
        }
    }

    pub fn blocks_test(&self) -> bool {
        matches!(
            self.status,
            RequirementStatus::Missing | RequirementStatus::Failed | RequirementStatus::Unverified
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanCalibrationReadiness {
    pub index: u32,
    pub name: String,
    pub role: FanRole,
    pub logical_channel_identified: bool,
    pub physical_identity: PhysicalIdentity,
    pub independent_rpm_mapped: bool,
    pub actuator_response_validated: bool,
    pub ready: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationProtocol {
    pub experimental_min_percent: u8,
    pub cpu_max_temp_c: f64,
    pub gpu_max_temp_c: f64,
    pub step_duration_ms: u64,
    pub single_fan_only: bool,
    pub other_fan_automatic_required: bool,
    pub automatic_recovery_required: bool,
    pub separate_authorization_required: bool,
    /// Always false. This report is evidence evaluation, not authorization.
    pub test_authorized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationReadiness {
    pub schema: String,
    pub generated_at_ms: u64,
    pub state: CalibrationState,
    /// This report never authorizes or starts a test.
    pub informational_only: bool,
    /// Always false. A separate explicit authorization remains mandatory.
    pub test_authorized: bool,
    pub requirements: Vec<CalibrationRequirement>,
    pub fans: Vec<FanCalibrationReadiness>,
    /// Missing or failed essential prerequisites.
    pub blockers: Vec<String>,
    /// Every requirement that is not satisfied, including unverified evidence.
    pub missing_evidence: Vec<String>,
    pub next_action: String,
    pub protocol: CalibrationProtocol,
}

impl CalibrationReadiness {
    pub fn requirement(&self, id: CalibrationRequirementId) -> Option<&CalibrationRequirement> {
        self.requirements
            .iter()
            .find(|requirement| requirement.id == id)
    }
}

pub fn evaluate(view: &RuntimeView) -> CalibrationReadiness {
    let control = control::assess_view(view);
    evaluate_with_control(view, &control)
}

pub fn evaluate_with_control(
    view: &RuntimeView,
    control: &ControlReadiness,
) -> CalibrationReadiness {
    let now = now_ms();
    let ttl = SafetyConfig::default().stale_after_ms;
    let status = view.status.as_ref();

    let runtime = runtime_requirement(view);
    let logical = logical_channel_requirement(status);
    let physical = physical_identity_requirement(control);
    let rpm = independent_rpm_requirement(view, now, ttl);
    let cpu = cpu_telemetry_requirement(&view.snapshot, now, ttl);
    let gpu = gpu_telemetry_requirement(&view.snapshot, status, now, ttl);
    let actuator = actuator_requirement(control);
    let recovery = recovery_requirement(view);
    let ceilings = temperature_ceiling_requirement();
    let floor = experimental_floor_requirement();

    let requirements = vec![
        runtime, logical, physical, rpm, cpu, gpu, actuator, recovery, ceilings, floor,
    ];

    let global_recovery_ok = requirements
        .iter()
        .find(|requirement| requirement.id == CalibrationRequirementId::RecoveryPath)
        .is_some_and(|requirement| requirement.status == RequirementStatus::Satisfied);
    let global_identity_ok = control.physical_identity_verified;
    let global_actuator_ok = control.physical_control_validated;

    let fans: Vec<FanCalibrationReadiness> = status
        .map(|status| {
            status
                .fans
                .iter()
                .map(|fan| {
                    let role = fan_role(&fan.name);
                    let logical_identified = role != FanRole::Unknown;
                    let physical_identity = if global_identity_ok {
                        PhysicalIdentity::PhysicallyVerified
                    } else if logical_identified {
                        PhysicalIdentity::LogicalOnly
                    } else {
                        PhysicalIdentity::Unknown
                    };
                    // No independent RPM-to-logical-fan mapping exists in the
                    // current evidence model. A global RPM channel is not
                    // enough to calibrate a specific physical fan.
                    let independent_rpm_mapped = false;
                    let mut reasons = Vec::new();
                    if !logical_identified {
                        reasons.push("logical channel name/index is ambiguous".to_string());
                    }
                    if !global_identity_ok {
                        reasons.push(
                            "physical fan identity is not independently verified".to_string(),
                        );
                    }
                    if !independent_rpm_mapped {
                        reasons.push(
                            "no independent RPM reference is mapped to this logical fan"
                                .to_string(),
                        );
                    }
                    if !global_actuator_ok {
                        reasons.push("actuator response is not validated".to_string());
                    }
                    if !global_recovery_ok {
                        reasons.push("automatic recovery path is not verified".to_string());
                    }
                    let ready = reasons.is_empty()
                        && logical_identified
                        && global_identity_ok
                        && independent_rpm_mapped
                        && global_actuator_ok
                        && global_recovery_ok;
                    FanCalibrationReadiness {
                        index: fan.index,
                        name: fan.name.clone(),
                        role,
                        logical_channel_identified: logical_identified,
                        physical_identity,
                        independent_rpm_mapped,
                        actuator_response_validated: global_actuator_ok,
                        ready,
                        reasons,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let fans_ready = !fans.is_empty() && fans.iter().all(|fan| fan.ready);

    let blockers: Vec<String> = requirements
        .iter()
        .filter(|requirement| {
            matches!(
                requirement.status,
                RequirementStatus::Missing | RequirementStatus::Failed
            )
        })
        .map(|requirement| format!("{}: {}", requirement.label, requirement.detail))
        .collect();
    let missing_evidence: Vec<String> = requirements
        .iter()
        .filter(|requirement| requirement.blocks_test())
        .map(|requirement| format!("{}: {}", requirement.label, requirement.detail))
        .collect();
    let state = derive_state(&requirements, fans_ready);

    let next_action = match state {
        CalibrationState::Blocked => format!(
            "Resolve the blocking calibration prerequisites ({}). This report does not authorize a test.",
            blockers
                .iter()
                .map(|blocker| blocker.split(':').next().unwrap_or(blocker))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CalibrationState::PartiallyReady => {
            "Record the remaining independent identity, RPM, actuator-response and recovery evidence. A separate explicit authorization is still mandatory."
                .to_string()
        }
        CalibrationState::ReadyForSeparatelyAuthorizedTest => {
            "All recorded prerequisites are satisfied. This is informational only; a separate explicit authorization is still mandatory before any test."
                .to_string()
        }
    };

    CalibrationReadiness {
        schema: SCHEMA.to_string(),
        generated_at_ms: now,
        state,
        informational_only: true,
        test_authorized: false,
        requirements,
        fans,
        blockers,
        missing_evidence,
        next_action,
        protocol: CalibrationProtocol {
            experimental_min_percent: EXPERIMENTAL_MIN_PERCENT,
            cpu_max_temp_c: CPU_MAX_TEMP_C,
            gpu_max_temp_c: GPU_MAX_TEMP_C,
            step_duration_ms: ValidationLimits::default().step_duration_ms,
            single_fan_only: true,
            other_fan_automatic_required: true,
            automatic_recovery_required: true,
            separate_authorization_required: true,
            test_authorized: false,
        },
    }
}

fn derive_state(requirements: &[CalibrationRequirement], fans_ready: bool) -> CalibrationState {
    let blocked = requirements.iter().any(|requirement| {
        matches!(
            requirement.status,
            RequirementStatus::Missing | RequirementStatus::Failed
        )
    });
    if blocked {
        return CalibrationState::Blocked;
    }
    let unverified = requirements
        .iter()
        .any(|requirement| requirement.status == RequirementStatus::Unverified);
    if unverified || !fans_ready {
        return CalibrationState::PartiallyReady;
    }
    CalibrationState::ReadyForSeparatelyAuthorizedTest
}

fn runtime_requirement(view: &RuntimeView) -> CalibrationRequirement {
    let Some(runtime) = &view.nbfc_runtime else {
        return CalibrationRequirement::new(
            CalibrationRequirementId::RuntimeBackend,
            RequirementStatus::Missing,
            "the supervisor did not report runtime EC-backend evidence; the installed build may predate the probe",
            "NBFC process descriptor / runtime probe",
        );
    };
    match (runtime.effective_backend, runtime.confidence) {
        (Some(backend), crate::nbfc_runtime::BackendConfidence::Verified) => {
            let endpoint = runtime
                .endpoints
                .first()
                .map(|endpoint| format!("fd {} -> {}", endpoint.fd, endpoint.path))
                .unwrap_or_else(|| "open descriptor not reported".to_string());
            CalibrationRequirement::new(
                CalibrationRequirementId::RuntimeBackend,
                RequirementStatus::Satisfied,
                format!("{backend} verified from the NBFC process descriptor ({endpoint})"),
                "NBFC process descriptor",
            )
        }
        (Some(backend), _) => CalibrationRequirement::new(
            CalibrationRequirementId::RuntimeBackend,
            RequirementStatus::Unverified,
            format!(
                "{backend} is configured but was not independently observed from the running process"
            ),
            "NBFC command line/configuration",
        ),
        (None, _) => CalibrationRequirement::new(
            CalibrationRequirementId::RuntimeBackend,
            RequirementStatus::Missing,
            runtime
                .probe_errors
                .first()
                .cloned()
                .unwrap_or_else(|| "runtime backend is unknown".to_string()),
            "NBFC process descriptor / runtime probe",
        ),
    }
}

fn logical_channel_requirement(status: Option<&NbfcStatus>) -> CalibrationRequirement {
    let Some(status) = status else {
        return CalibrationRequirement::new(
            CalibrationRequirementId::LogicalChannel,
            RequirementStatus::Missing,
            "NBFC status is unavailable",
            "NBFC logical fan status",
        );
    };
    if status.fans.is_empty() {
        return CalibrationRequirement::new(
            CalibrationRequirementId::LogicalChannel,
            RequirementStatus::Missing,
            "NBFC reports no logical fan channels",
            "NBFC logical fan status",
        );
    }
    let roles: Vec<FanRole> = status.fans.iter().map(|fan| fan_role(&fan.name)).collect();
    if roles.contains(&FanRole::Unknown) {
        return CalibrationRequirement::new(
            CalibrationRequirementId::LogicalChannel,
            RequirementStatus::Failed,
            "one or more logical fan names have an ambiguous CPU/GPU role",
            "NBFC logical fan names/indexes",
        );
    }
    if !roles.contains(&FanRole::Cpu) || !roles.contains(&FanRole::Gpu) {
        return CalibrationRequirement::new(
            CalibrationRequirementId::LogicalChannel,
            RequirementStatus::Unverified,
            "both CPU and GPU logical channels are not present",
            "NBFC logical fan names/indexes",
        );
    }
    CalibrationRequirement::new(
        CalibrationRequirementId::LogicalChannel,
        RequirementStatus::Satisfied,
        format!(
            "{} logical channel(s): {}",
            status.fans.len(),
            status
                .fans
                .iter()
                .map(|fan| format!("{}={}", fan.index, fan.name))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        "NBFC logical fan names/indexes; not physical identity",
    )
}

fn physical_identity_requirement(control: &ControlReadiness) -> CalibrationRequirement {
    if control.physical_identity_verified {
        CalibrationRequirement::new(
            CalibrationRequirementId::PhysicalIdentity,
            RequirementStatus::Satisfied,
            "a per-fan physical identity record is present",
            "external airflow/tachometer mapping evidence",
        )
    } else {
        CalibrationRequirement::new(
            CalibrationRequirementId::PhysicalIdentity,
            RequirementStatus::Unverified,
            "no independent per-fan physical identity evidence is recorded; logical names/registers are not proof",
            "external airflow/tachometer mapping evidence",
        )
    }
}

fn independent_rpm_requirement(view: &RuntimeView, now: u64, ttl: u64) -> CalibrationRequirement {
    let channels: Vec<_> = view.snapshot.rpm_sensors().collect();
    let verified = channels.iter().find(|sensor| {
        sensor.unit == Unit::Rpm
            && sensor.health.trust.is_at_least(Trust::Verified)
            && sensor.health.physical_measurement
            && sensor.is_usable_for_feedback(now, ttl)
    });
    if let Some(sensor) = verified {
        return CalibrationRequirement::new(
            CalibrationRequirementId::IndependentRpm,
            RequirementStatus::Satisfied,
            format!(
                "{} reports {} as fresh trusted feedback (unit {})",
                sensor.display_name(),
                sensor.display_value(),
                format!("{:?}", sensor.unit).to_ascii_lowercase()
            ),
            sensor.path.as_deref().unwrap_or("physical RPM sensor"),
        );
    }
    let provisional = channels
        .iter()
        .any(|sensor| sensor.unit == Unit::Raw && sensor.health.trust == Trust::Provisional);
    if provisional {
        CalibrationRequirement::new(
            CalibrationRequirementId::IndependentRpm,
            RequirementStatus::Missing,
            "only provisional raw EC tach words are present; they are not RPM and have no validated unit",
            "external tachometer or calibrated airflow reference",
        )
    } else {
        CalibrationRequirement::new(
            CalibrationRequirementId::IndependentRpm,
            RequirementStatus::Missing,
            "no independent tachometer, RPM counter or calibrated airflow reference is available; ACPI fan cooling state is a binary power state, not RPM",
            "external tachometer or calibrated airflow reference",
        )
    }
}

fn cpu_telemetry_requirement(
    snapshot: &SensorSnapshot,
    now: u64,
    ttl: u64,
) -> CalibrationRequirement {
    match snapshot.cpu_package(now, ttl) {
        Some(sensor) => CalibrationRequirement::new(
            CalibrationRequirementId::CpuTelemetry,
            RequirementStatus::Satisfied,
            format!(
                "{} ({}, {})",
                sensor.display_value(),
                sensor.health.trust.short_label(),
                if sensor.is_fresh(now, ttl) {
                    "fresh"
                } else {
                    "stale"
                }
            ),
            sensor.path.as_deref().unwrap_or("CPU temperature provider"),
        ),
        None => CalibrationRequirement::new(
            CalibrationRequirementId::CpuTelemetry,
            RequirementStatus::Missing,
            "no valid, fresh and trusted CPU package temperature",
            "coretemp/x86_pkg_temp",
        ),
    }
}

fn gpu_telemetry_requirement(
    snapshot: &SensorSnapshot,
    status: Option<&NbfcStatus>,
    now: u64,
    ttl: u64,
) -> CalibrationRequirement {
    let gpu_fan_configured = status.is_some_and(|status| {
        status
            .fans
            .iter()
            .any(|fan| fan_role(&fan.name) == FanRole::Gpu)
    });
    if !gpu_fan_configured {
        return CalibrationRequirement::new(
            CalibrationRequirementId::GpuTelemetry,
            RequirementStatus::NotApplicable,
            "no GPU logical fan is configured",
            "NBFC logical fan list",
        );
    }
    match snapshot
        .gpu_indexes()
        .into_iter()
        .find_map(|index| snapshot.gpu_die(index, now, ttl))
    {
        Some(sensor) => CalibrationRequirement::new(
            CalibrationRequirementId::GpuTelemetry,
            RequirementStatus::Satisfied,
            format!(
                "{} ({}, {})",
                sensor.display_value(),
                sensor.health.trust.short_label(),
                if sensor.is_fresh(now, ttl) {
                    "fresh"
                } else {
                    "stale"
                }
            ),
            sensor.path.as_deref().unwrap_or("GPU temperature provider"),
        ),
        None => CalibrationRequirement::new(
            CalibrationRequirementId::GpuTelemetry,
            RequirementStatus::Missing,
            "a GPU logical fan exists but no valid, fresh and trusted GPU die temperature is available",
            "NVIDIA/GPU temperature provider",
        ),
    }
}

fn actuator_requirement(control: &ControlReadiness) -> CalibrationRequirement {
    if control.physical_control_validated {
        CalibrationRequirement::new(
            CalibrationRequirementId::ActuatorSemantics,
            RequirementStatus::Satisfied,
            "a validated single-fan actuator-response record is present",
            "controlled actuator-response evidence",
        )
    } else {
        CalibrationRequirement::new(
            CalibrationRequirementId::ActuatorSemantics,
            RequirementStatus::Unverified,
            "backend target acceptance is not physical actuator validation; no single-fan response record is present",
            "controlled actuator-response evidence",
        )
    }
}

fn recovery_requirement(view: &RuntimeView) -> CalibrationRequirement {
    let recovery = view
        .last_validation
        .as_ref()
        .and_then(|session| session.recovery.as_ref());
    match recovery {
        Some(recovery)
            if recovery.attempted
                && recovery.backend_auto
                && recovery.ec_verified == Some(true) =>
        {
            CalibrationRequirement::new(
                CalibrationRequirementId::RecoveryPath,
                RequirementStatus::Satisfied,
                "automatic recovery was attempted and both backend and raw EC state were verified",
                "recorded validation recovery evidence",
            )
        }
        Some(recovery) if recovery.attempted && recovery.backend_auto => {
            CalibrationRequirement::new(
                CalibrationRequirementId::RecoveryPath,
                RequirementStatus::Unverified,
                format!(
                    "backend automatic recovery was recorded, but EC automatic mode was not verified: {}",
                    recovery.detail
                ),
                "recorded validation recovery evidence",
            )
        }
        Some(recovery) if recovery.attempted => CalibrationRequirement::new(
            CalibrationRequirementId::RecoveryPath,
            RequirementStatus::Failed,
            format!(
                "automatic recovery was attempted but not confirmed: {}",
                recovery.detail
            ),
            "recorded validation recovery evidence",
        ),
        _ => CalibrationRequirement::new(
            CalibrationRequirementId::RecoveryPath,
            RequirementStatus::Unverified,
            "no recorded automatic-recovery evidence is available",
            "supervisor validation recovery evidence",
        ),
    }
}

fn temperature_ceiling_requirement() -> CalibrationRequirement {
    let limits = ValidationLimits::default();
    let valid = limits.validate().is_ok() && CPU_MAX_TEMP_C <= 80.0 && GPU_MAX_TEMP_C <= 75.0;
    CalibrationRequirement::new(
        CalibrationRequirementId::TemperatureCeilings,
        if valid {
            RequirementStatus::Satisfied
        } else {
            RequirementStatus::Failed
        },
        format!(
            "CPU ceiling {CPU_MAX_TEMP_C:.0} °C, GPU ceiling {GPU_MAX_TEMP_C:.0} °C; configured limits validate={}",
            limits.validate().is_ok()
        ),
        "compiled validation limits",
    )
}

fn experimental_floor_requirement() -> CalibrationRequirement {
    let valid = EXPERIMENTAL_MIN_PERCENT == 50
        && ValidationStep::SEQUENCE
            .iter()
            .all(|step| step.percent() >= f64::from(EXPERIMENTAL_MIN_PERCENT))
        && !ValidationStep::SEQUENCE.contains(&ValidationStep::Zero);
    CalibrationRequirement::new(
        CalibrationRequirementId::ExperimentalFloor,
        if valid {
            RequirementStatus::Satisfied
        } else {
            RequirementStatus::Failed
        },
        format!(
            "experimental minimum {}%; sequence {:?}",
            EXPERIMENTAL_MIN_PERCENT,
            ValidationStep::SEQUENCE
                .iter()
                .map(|step| step.label())
                .collect::<Vec<_>>()
        ),
        "compiled validation state machine",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SensorKind, SensorProvider, SensorRole, SensorSample};
    use crate::nbfc::{NbfcCapabilities, NbfcFan, NbfcVersion};
    use crate::nbfc_runtime::{BackendConfidence, EcBackend, NbfcRuntimeInfo, RuntimeEndpoint};
    use crate::service::ServiceStatus;
    use crate::supervisor::SupervisorProbe;
    use crate::validation::RecoveryEvidence;

    fn sample(
        role: SensorRole,
        kind: SensorKind,
        value: f64,
        unit: Unit,
        provider: SensorProvider,
        trust: Trust,
    ) -> SensorSample {
        SensorSample::new(
            format!("test:{role}"),
            role,
            kind,
            Some(value),
            unit,
            Some(value.to_string()),
            "test",
            provider,
            "test".into(),
            Some("/sys/test".into()),
            None,
            None,
        )
        .with_trust(trust)
    }

    fn status() -> NbfcStatus {
        NbfcStatus {
            version: None,
            read_only: Some(false),
            selected_config: Some("Test".into()),
            fans: vec![
                NbfcFan {
                    index: 0,
                    name: "CPU fan".into(),
                    temperature: Some(55.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(true),
                    critical: Some(false),
                    current_speed: Some(100.0),
                    target_speed: Some(35.0),
                    requested_speed: Some(-1.0),
                    speed_steps: Some(100),
                },
                NbfcFan {
                    index: 1,
                    name: "GPU fan".into(),
                    temperature: Some(55.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(true),
                    critical: Some(false),
                    current_speed: Some(100.0),
                    target_speed: Some(40.0),
                    requested_speed: Some(-1.0),
                    speed_steps: Some(100),
                },
            ],
            warnings: Vec::new(),
        }
    }

    fn runtime() -> NbfcRuntimeInfo {
        NbfcRuntimeInfo {
            schema: crate::nbfc_runtime::SCHEMA.to_string(),
            pid: Some(2393),
            command_line: vec!["nbfc_service".into(), "-f".into()],
            config_path: Some("/etc/nbfc/nbfc.json".into()),
            configured_backend: None,
            observed_backend: Some(EcBackend::EcSys),
            effective_backend: Some(EcBackend::EcSys),
            confidence: BackendConfidence::Verified,
            endpoints: vec![RuntimeEndpoint {
                fd: 18,
                path: "/sys/kernel/debug/ec/ec0/io".into(),
                backend: EcBackend::EcSys,
            }],
            evidence: vec!["fd 18 open".into()],
            probe_errors: Vec::new(),
        }
    }

    fn view() -> RuntimeView {
        let version = NbfcVersion::parse("nbfc 0.5.4").unwrap();
        RuntimeView {
            snapshot: SensorSnapshot {
                sensors: vec![
                    sample(
                        SensorRole::CpuPackage,
                        SensorKind::Temperature,
                        55.0,
                        Unit::Celsius,
                        SensorProvider::Hwmon,
                        Trust::Verified,
                    ),
                    sample(
                        SensorRole::GpuDie(0),
                        SensorKind::Temperature,
                        50.0,
                        Unit::Celsius,
                        SensorProvider::Nvidia,
                        Trust::Verified,
                    ),
                ],
                errors: Vec::new(),
                collected_at_ms: now_ms(),
            },
            sensor_source_error: None,
            version: Some(version),
            capabilities: Some(NbfcCapabilities::from_version(version)),
            status: Some(status()),
            nbfc_runtime: Some(runtime()),
            last_validation: None,
            service: ServiceStatus {
                active_state: Some("active".into()),
                ..ServiceStatus::default()
            },
            sensor_config: None,
            backend_error: None,
            supervisor: SupervisorProbe {
                state: crate::supervisor::SupervisorState::Healthy,
                socket: "/run/neosense/neosensed.sock".into(),
                socket_exists: true,
                socket_mode: Some("660".into()),
                socket_uid: Some(0),
                socket_gid: Some(964),
                health: Some(true),
                reason: None,
                error: None,
                status: None,
            },
        }
    }

    #[test]
    fn missing_runtime_and_independent_rpm_keep_readiness_blocked() {
        let mut view = view();
        view.nbfc_runtime = None;
        let readiness = evaluate(&view);
        assert_eq!(readiness.state, CalibrationState::Blocked);
        assert_eq!(readiness.requirements.len(), 10);
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::RuntimeBackend)
                .unwrap()
                .status,
            RequirementStatus::Missing
        );
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::IndependentRpm)
                .unwrap()
                .status,
            RequirementStatus::Missing
        );
        assert!(!readiness.test_authorized);
        assert!(readiness.informational_only);
        assert!(
            readiness
                .fans
                .iter()
                .all(|fan| !fan.ready && !fan.independent_rpm_mapped)
        );
    }

    #[test]
    fn verified_backend_and_telemetry_are_not_enough_for_physical_readiness() {
        let readiness = evaluate(&view());
        assert_eq!(readiness.state, CalibrationState::Blocked);
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::RuntimeBackend)
                .unwrap()
                .status,
            RequirementStatus::Satisfied
        );
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::CpuTelemetry)
                .unwrap()
                .status,
            RequirementStatus::Satisfied
        );
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::GpuTelemetry)
                .unwrap()
                .status,
            RequirementStatus::Satisfied
        );
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::PhysicalIdentity)
                .unwrap()
                .status,
            RequirementStatus::Unverified
        );
        assert!(
            readiness
                .missing_evidence
                .iter()
                .any(|evidence| evidence.contains("physical identity"))
        );
    }

    #[test]
    fn a_verified_rpm_channel_without_mapping_is_not_per_fan_ready() {
        let mut view = view();
        view.snapshot.sensors.push(sample(
            SensorRole::FanRpm(0),
            SensorKind::Rpm,
            3000.0,
            Unit::Rpm,
            SensorProvider::Hwmon,
            Trust::Verified,
        ));
        let readiness = evaluate(&view);
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::IndependentRpm)
                .unwrap()
                .status,
            RequirementStatus::Satisfied
        );
        assert!(
            readiness
                .fans
                .iter()
                .all(|fan| !fan.independent_rpm_mapped && !fan.ready)
        );
        assert_ne!(
            readiness.state,
            CalibrationState::ReadyForSeparatelyAuthorizedTest
        );
    }

    #[test]
    fn recovery_evidence_is_read_only_and_strict() {
        let mut view = view();
        let mut session = crate::validation::ValidationSession::new(
            "s".into(),
            crate::validation::ValidationFan {
                index: 0,
                backend_id: "nbfc:0".into(),
                name: "CPU fan".into(),
                role: FanRole::Cpu,
            },
            crate::validation::ExpectedSide::Left,
            ValidationLimits::default(),
            now_ms(),
            Some("Test".into()),
            Some("0.5.4".into()),
        );
        session.recovery = Some(RecoveryEvidence {
            attempted: true,
            backend_auto: true,
            ec_verified: Some(false),
            detail: "backend automatic; EC unverified".into(),
            completed_at_ms: 1,
        });
        view.last_validation = Some(session);
        let readiness = evaluate(&view);
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::RecoveryPath)
                .unwrap()
                .status,
            RequirementStatus::Unverified
        );

        let mut session = view.last_validation.clone().unwrap();
        session.recovery.as_mut().unwrap().ec_verified = Some(true);
        view.last_validation = Some(session);
        let readiness = evaluate(&view);
        assert_eq!(
            readiness
                .requirement(CalibrationRequirementId::RecoveryPath)
                .unwrap()
                .status,
            RequirementStatus::Satisfied
        );
    }

    #[test]
    fn state_derivation_is_fail_closed() {
        let satisfied = vec![
            CalibrationRequirement::new(
                CalibrationRequirementId::RuntimeBackend,
                RequirementStatus::Satisfied,
                "ok",
                "test",
            ),
            CalibrationRequirement::new(
                CalibrationRequirementId::IndependentRpm,
                RequirementStatus::Satisfied,
                "ok",
                "test",
            ),
        ];
        assert_eq!(
            derive_state(&satisfied, false),
            CalibrationState::PartiallyReady
        );
        assert_eq!(
            derive_state(&satisfied, true),
            CalibrationState::ReadyForSeparatelyAuthorizedTest
        );
        let mut missing = satisfied.clone();
        missing[0].status = RequirementStatus::Missing;
        assert_eq!(derive_state(&missing, true), CalibrationState::Blocked);
        let mut unverified = satisfied.clone();
        unverified[0].status = RequirementStatus::Unverified;
        assert_eq!(
            derive_state(&unverified, true),
            CalibrationState::PartiallyReady
        );
    }
}
