//! Read-only control-readiness assessment.
//!
//! This module answers a narrower question than diagnostics: *why* is hardware
//! control eligible or not, and which evidence is missing? It is pure: it never
//! writes hardware, starts a backend, or treats backend acceptance as physical
//! validation. The Acer physical-control state remains unvalidated until
//! independent evidence is recorded outside this module.

use crate::model::{SensorRole, SensorSnapshot, now_ms};
use crate::nbfc::{NbfcCapabilities, NbfcSensorConfig, NbfcStatus};
use crate::policy::{FanRole, SafetyConfig, SafetyState, fan_role};
use crate::regulation::{RegulationMode, mapping_matches};
use crate::runtime::RuntimeView;
use crate::supervisor::SupervisorState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlStage {
    #[default]
    /// No supervisor/backend state is available.
    Unavailable,
    /// A backend is visible, but control prerequisites are not all satisfied.
    HardwareDetected,
    /// Control prerequisites are satisfied but validation evidence is missing.
    HardwareEligible,
    /// Control prerequisites are satisfied; physical validation is incomplete.
    HardwareUnvalidated,
    /// Every control and validation prerequisite is satisfied.
    HardwareValidated,
}

impl ControlStage {
    pub fn label(self) -> &'static str {
        match self {
            ControlStage::Unavailable => "unavailable",
            ControlStage::HardwareDetected => "hardware detected",
            ControlStage::HardwareEligible => "hardware eligible",
            ControlStage::HardwareUnvalidated => "hardware unvalidated",
            ControlStage::HardwareValidated => "hardware validated",
        }
    }

    pub fn hardware_control_validated(self) -> bool {
        self == ControlStage::HardwareValidated
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    Automatic,
    Manual,
    Failsafe,
    Degraded,
    #[default]
    Unavailable,
}

impl ControlMode {
    pub fn label(self) -> &'static str {
        match self {
            ControlMode::Automatic => "automatic",
            ControlMode::Manual => "manual",
            ControlMode::Failsafe => "failsafe",
            ControlMode::Degraded => "degraded",
            ControlMode::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementScope {
    Control,
    Validation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementStatus {
    Satisfied,
    Missing,
    Failed,
    Unverified,
    NotApplicable,
}

impl RequirementStatus {
    pub fn is_blocking(self) -> bool {
        matches!(self, RequirementStatus::Missing | RequirementStatus::Failed)
    }

    pub fn label(self) -> &'static str {
        match self {
            RequirementStatus::Satisfied => "satisfied",
            RequirementStatus::Missing => "missing",
            RequirementStatus::Failed => "failed",
            RequirementStatus::Unverified => "unverified",
            RequirementStatus::NotApplicable => "not applicable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequirementId {
    Supervisor,
    Backend,
    ProtectedManual,
    NbfcSocket,
    CpuTemperature,
    GpuTemperature,
    FanMapping,
    RpmFeedback,
    PhysicalIdentity,
    RpmCalibration,
    PhysicalControl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub id: RequirementId,
    pub label: &'static str,
    pub scope: RequirementScope,
    pub status: RequirementStatus,
    pub detail: String,
    pub evidence: String,
}

impl Requirement {
    fn new(
        id: RequirementId,
        label: &'static str,
        scope: RequirementScope,
        status: RequirementStatus,
        detail: impl Into<String>,
        evidence: impl Into<String>,
    ) -> Self {
        Self {
            id,
            label,
            scope,
            status,
            detail: detail.into(),
            evidence: evidence.into(),
        }
    }
}

/// Per-fan readiness. `policy_eligible` describes only the current control
/// prerequisites; `validation_unverified` lists physical evidence that is still
/// missing and must never be inferred from eligibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanReadiness {
    pub index: u32,
    pub name: String,
    pub role: FanRole,
    pub policy_eligible: bool,
    pub mapping_ok: bool,
    /// NBFC sensor sources for this logical fan (for example `["@CPU"]`).
    pub sources: Vec<String>,
    pub temperature_status: RequirementStatus,
    /// Explicit reasons this fan is not policy-eligible. Empty when eligible.
    pub reasons: Vec<String>,
    /// Missing physical evidence that does not by itself change policy
    /// eligibility.
    pub validation_unverified: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlReadiness {
    pub schema: &'static str,
    pub generated_at_ms: u64,
    pub stage: ControlStage,
    pub mode: ControlMode,
    /// At least one configured fan has a policy-eligible manual path. This is
    /// not physical validation.
    pub manual_path_eligible: bool,
    /// Every configured fan has a policy-eligible manual path. This is still
    /// not physical validation.
    pub all_fans_eligible: bool,
    pub eligible_fans: Vec<u32>,
    pub blocked_fans: Vec<u32>,
    /// One entry per configured logical fan, with explicit blockers and
    /// validation evidence.
    #[serde(default)]
    pub fans: Vec<FanReadiness>,
    pub physical_identity_verified: bool,
    pub rpm_calibration_verified: bool,
    pub physical_control_validated: bool,
    /// Selected closed-loop profile mode, if any. Preview/simulation never
    /// imply hardware execution.
    #[serde(default)]
    pub profile_mode: Option<RegulationMode>,
    pub requirements: Vec<Requirement>,
    /// Missing or failed control requirements. Unverified validation evidence
    /// is tracked separately and is not a control blocker.
    pub blockers: Vec<String>,
    pub next_action: String,
}

impl ControlReadiness {
    pub fn requirements_in(&self, scope: RequirementScope) -> impl Iterator<Item = &Requirement> {
        self.requirements
            .iter()
            .filter(move |requirement| requirement.scope == scope)
    }

    pub fn requirement(&self, id: RequirementId) -> Option<&Requirement> {
        self.requirements
            .iter()
            .find(|requirement| requirement.id == id)
    }
}

pub struct ReadinessInput<'a> {
    pub supervisor: SupervisorState,
    pub snapshot: &'a SensorSnapshot,
    pub capabilities: Option<&'a NbfcCapabilities>,
    pub status: Option<&'a NbfcStatus>,
    pub sensor_config: Option<&'a NbfcSensorConfig>,
    pub physical_identity_verified: bool,
    pub rpm_calibration_verified: bool,
    pub physical_control_validated: bool,
    pub profile_mode: Option<RegulationMode>,
    pub safety_state: Option<SafetyState>,
    pub config: &'a SafetyConfig,
}

fn supervisor_requirement(state: SupervisorState) -> Requirement {
    let (status, detail) = match state {
        SupervisorState::Healthy => (RequirementStatus::Satisfied, "supervisor healthy"),
        SupervisorState::Connected => (
            RequirementStatus::Satisfied,
            "supervisor connected but health was not reported",
        ),
        SupervisorState::Degraded => (
            RequirementStatus::Failed,
            "supervisor reports degraded health",
        ),
        SupervisorState::NotRunning => (RequirementStatus::Missing, "supervisor is not running"),
        SupervisorState::PermissionDenied => (
            RequirementStatus::Failed,
            "supervisor socket is not accessible to this session",
        ),
        SupervisorState::StaleSocket => (
            RequirementStatus::Missing,
            "supervisor socket exists but no process is listening",
        ),
        SupervisorState::ProtocolError => {
            (RequirementStatus::Failed, "supervisor IPC protocol error")
        }
    };
    Requirement::new(
        RequirementId::Supervisor,
        "Supervisor IPC",
        RequirementScope::Control,
        status,
        detail,
        "supervisor status probe",
    )
}

fn backend_requirement(
    capabilities: Option<&NbfcCapabilities>,
    status: Option<&NbfcStatus>,
) -> Requirement {
    match (capabilities, status) {
        (Some(capabilities), Some(_)) => Requirement::new(
            RequirementId::Backend,
            "NBFC backend",
            RequirementScope::Control,
            RequirementStatus::Satisfied,
            format!("NBFC {} status available", capabilities.version),
            "supervisor status document",
        ),
        (Some(capabilities), None) => Requirement::new(
            RequirementId::Backend,
            "NBFC backend",
            RequirementScope::Control,
            RequirementStatus::Failed,
            format!(
                "NBFC {} version is known but its status could not be read",
                capabilities.version
            ),
            "supervisor status document",
        ),
        _ => Requirement::new(
            RequirementId::Backend,
            "NBFC backend",
            RequirementScope::Control,
            RequirementStatus::Missing,
            "NBFC version or status is unavailable",
            "supervisor status document",
        ),
    }
}

fn protected_manual_requirement(capabilities: Option<&NbfcCapabilities>) -> Requirement {
    match capabilities {
        Some(capabilities) if capabilities.protected_manual && capabilities.access_controlled => {
            Requirement::new(
                RequirementId::ProtectedManual,
                "Protected manual path",
                RequirementScope::Control,
                RequirementStatus::Satisfied,
                format!(
                    "NBFC {} provides protected manual control with access control",
                    capabilities.version
                ),
                "NBFC version and socket access-control classification",
            )
        }
        Some(capabilities) if !capabilities.protected_manual => Requirement::new(
            RequirementId::ProtectedManual,
            "Protected manual path",
            RequirementScope::Control,
            RequirementStatus::Failed,
            format!(
                "NBFC {} does not provide protected manual control (>= 0.5.4 required)",
                capabilities.version
            ),
            "NBFC version",
        ),
        Some(_) => Requirement::new(
            RequirementId::ProtectedManual,
            "Protected manual path",
            RequirementScope::Control,
            RequirementStatus::Failed,
            "NBFC socket is not access-controlled",
            "supervisor socket inspection",
        ),
        None => Requirement::new(
            RequirementId::ProtectedManual,
            "Protected manual path",
            RequirementScope::Control,
            RequirementStatus::Missing,
            "NBFC capabilities are unavailable",
            "supervisor status document",
        ),
    }
}

fn socket_requirement(capabilities: Option<&NbfcCapabilities>) -> Requirement {
    match capabilities.and_then(|capabilities| capabilities.socket.as_ref()) {
        Some(socket) if socket.access_controlled => Requirement::new(
            RequirementId::NbfcSocket,
            "NBFC socket permissions",
            RequirementScope::Control,
            RequirementStatus::Satisfied,
            format!(
                "{} mode {}; no unverified non-root writer",
                socket.path,
                socket.mode.as_deref().unwrap_or("unknown")
            ),
            format!("socket identity source: {}", socket.identity_source),
        ),
        Some(socket) => Requirement::new(
            RequirementId::NbfcSocket,
            "NBFC socket permissions",
            RequirementScope::Control,
            RequirementStatus::Failed,
            format!(
                "{} mode {} is not access-controlled",
                socket.path,
                socket.mode.as_deref().unwrap_or("unknown")
            ),
            "supervisor socket inspection",
        ),
        None => Requirement::new(
            RequirementId::NbfcSocket,
            "NBFC socket permissions",
            RequirementScope::Control,
            RequirementStatus::Missing,
            "socket permissions were not reported by the supervisor",
            "supervisor status document",
        ),
    }
}

fn cpu_temperature_requirement(
    snapshot: &SensorSnapshot,
    now: u64,
    config: &SafetyConfig,
) -> Requirement {
    if let Some(sensor) = snapshot.cpu_package(now, config.stale_after_ms) {
        return Requirement::new(
            RequirementId::CpuTemperature,
            "CPU temperature",
            RequirementScope::Control,
            RequirementStatus::Satisfied,
            format!(
                "{} ({} °C, {})",
                sensor.display_name(),
                sensor.display_value(),
                sensor.health.trust.short_label()
            ),
            sensor.path.clone().unwrap_or_else(|| sensor.id.clone()),
        );
    }
    let present = snapshot
        .sensors
        .iter()
        .any(|sensor| sensor.role == SensorRole::CpuPackage);
    Requirement::new(
        RequirementId::CpuTemperature,
        "CPU temperature",
        RequirementScope::Control,
        if present {
            RequirementStatus::Failed
        } else {
            RequirementStatus::Missing
        },
        if present {
            "a CPU package channel exists but is not valid, fresh and trusted"
        } else {
            "no CPU package temperature source was discovered"
        },
        "sensor snapshot",
    )
}

fn gpu_temperature_requirement(
    snapshot: &SensorSnapshot,
    status: Option<&NbfcStatus>,
    now: u64,
    config: &SafetyConfig,
) -> Requirement {
    let needs_gpu = status.is_some_and(|status| {
        status
            .fans
            .iter()
            .any(|fan| fan_role(&fan.name) == FanRole::Gpu)
    });
    if !needs_gpu {
        return Requirement::new(
            RequirementId::GpuTemperature,
            "GPU temperature",
            RequirementScope::Control,
            RequirementStatus::NotApplicable,
            "no GPU-role fan is configured",
            "NBFC fan names",
        );
    }
    if let Some(sensor) = snapshot
        .gpu_indexes()
        .into_iter()
        .find_map(|index| snapshot.gpu_die(index, now, config.stale_after_ms))
    {
        return Requirement::new(
            RequirementId::GpuTemperature,
            "GPU temperature",
            RequirementScope::Control,
            RequirementStatus::Satisfied,
            format!(
                "{} ({} °C, {})",
                sensor.display_name(),
                sensor.display_value(),
                sensor.health.trust.short_label()
            ),
            sensor.path.clone().unwrap_or_else(|| sensor.id.clone()),
        );
    }
    let present = snapshot
        .sensors
        .iter()
        .any(|sensor| matches!(sensor.role, SensorRole::GpuDie(_)));
    Requirement::new(
        RequirementId::GpuTemperature,
        "GPU temperature",
        RequirementScope::Control,
        if present {
            RequirementStatus::Failed
        } else {
            RequirementStatus::Missing
        },
        if present {
            "a GPU die channel exists but is not valid, fresh and trusted"
        } else {
            "no GPU temperature source was discovered"
        },
        "sensor snapshot",
    )
}

fn fan_mapping_requirement(
    status: Option<&NbfcStatus>,
    sensor_config: Option<&NbfcSensorConfig>,
) -> Requirement {
    let Some(status) = status else {
        return Requirement::new(
            RequirementId::FanMapping,
            "Fan sensor mapping",
            RequirementScope::Control,
            RequirementStatus::Missing,
            "NBFC fan status is unavailable",
            "supervisor status document",
        );
    };
    if status.fans.is_empty() {
        return Requirement::new(
            RequirementId::FanMapping,
            "Fan sensor mapping",
            RequirementScope::Control,
            RequirementStatus::Missing,
            "no NBFC fans were reported",
            "NBFC status",
        );
    }
    let Some(sensor_config) = sensor_config else {
        return Requirement::new(
            RequirementId::FanMapping,
            "Fan sensor mapping",
            RequirementScope::Control,
            RequirementStatus::Missing,
            "NBFC sensor sources are unavailable",
            "supervisor status document",
        );
    };
    let mut failures = Vec::new();
    for fan in &status.fans {
        let role = fan_role(&fan.name);
        let Some(source) = sensor_config
            .fans
            .iter()
            .find(|source| source.index == fan.index)
        else {
            failures.push(format!("{}: no sensor source", fan.name));
            continue;
        };
        if role == FanRole::Unknown {
            failures.push(format!("{}: fan role is ambiguous", fan.name));
        } else if !mapping_matches(role, &source.sensors) {
            failures.push(format!(
                "{}: sources {:?} do not match role {}; this is an NBFC sensor-source/configuration limitation, not proof of a physical fan mapping",
                fan.name, source.sensors, role
            ));
        }
    }
    if failures.is_empty() {
        Requirement::new(
            RequirementId::FanMapping,
            "Fan sensor mapping",
            RequirementScope::Control,
            RequirementStatus::Satisfied,
            format!(
                "{} fan mapping(s) match their declared role",
                status.fans.len()
            ),
            "NBFC status and sensor-source document",
        )
    } else {
        Requirement::new(
            RequirementId::FanMapping,
            "Fan sensor mapping",
            RequirementScope::Control,
            RequirementStatus::Failed,
            failures.join("; "),
            "NBFC status and sensor-source document",
        )
    }
}

fn rpm_feedback_requirement(
    snapshot: &SensorSnapshot,
    now: u64,
    config: &SafetyConfig,
) -> Requirement {
    let rpms: Vec<_> = snapshot.rpm_sensors().collect();
    if rpms.is_empty() {
        return Requirement::new(
            RequirementId::RpmFeedback,
            "RPM feedback",
            RequirementScope::Validation,
            RequirementStatus::Missing,
            "no independent tachometer channel was discovered",
            "sensor snapshot",
        );
    }
    let usable = rpms
        .iter()
        .filter(|sensor| sensor.is_usable_for_feedback(now, config.stale_after_ms))
        .count();
    if usable == 0 {
        return Requirement::new(
            RequirementId::RpmFeedback,
            "RPM feedback",
            RequirementScope::Validation,
            RequirementStatus::Failed,
            "tachometer channels exist but none is valid, fresh and trusted",
            "sensor snapshot",
        );
    }
    Requirement::new(
        RequirementId::RpmFeedback,
        "RPM feedback",
        RequirementScope::Validation,
        RequirementStatus::Satisfied,
        format!(
            "{usable}/{} tachometer channel(s) are fresh, trusted feedback",
            rpms.len()
        ),
        "sensor snapshot; feedback never enables control",
    )
}

fn validation_requirement(
    id: RequirementId,
    label: &'static str,
    verified: bool,
    evidence: &'static str,
) -> Requirement {
    Requirement::new(
        id,
        label,
        RequirementScope::Validation,
        if verified {
            RequirementStatus::Satisfied
        } else {
            RequirementStatus::Unverified
        },
        if verified {
            "explicit validation evidence is recorded"
        } else {
            "no explicit validation evidence is recorded; do not infer this from backend acceptance or RPM availability"
        },
        evidence,
    )
}

/// Per-fan policy eligibility. A fan needs a known role, a matching sensor
/// source, and a usable temperature for that role. This is deliberately
/// separate from physical identity/control validation.
#[allow(clippy::too_many_arguments)]
fn fan_readiness(
    status: Option<&NbfcStatus>,
    sensor_config: Option<&NbfcSensorConfig>,
    cpu: &Requirement,
    gpu: &Requirement,
    essential_control_ok: bool,
    identity: &Requirement,
    calibration: &Requirement,
    control: &Requirement,
) -> Vec<FanReadiness> {
    let Some(status) = status else {
        return Vec::new();
    };
    status
        .fans
        .iter()
        .map(|fan| {
            let role = fan_role(&fan.name);
            let source = sensor_config
                .and_then(|config| config.fans.iter().find(|source| source.index == fan.index));
            let sources = source.map(|source| source.sensors.clone()).unwrap_or_default();
            let mapping_ok = role != FanRole::Unknown
                && !sources.is_empty()
                && mapping_matches(role, &sources);
            let temperature = match role {
                FanRole::Cpu => Some(cpu),
                FanRole::Gpu => Some(gpu),
                FanRole::Unknown => None,
            };
            let temperature_status = temperature
                .map(|requirement| requirement.status)
                .unwrap_or(RequirementStatus::Missing);
            let mut reasons = Vec::new();
            if !essential_control_ok {
                reasons.push(
                    "supervisor, backend or access-control prerequisites are not all satisfied"
                        .to_string(),
                );
            }
            if role == FanRole::Unknown {
                reasons.push("fan role is ambiguous".to_string());
            }
            if sources.is_empty() {
                reasons.push("NBFC sensor source is missing".to_string());
            } else if !mapping_ok {
                reasons.push(format!(
                    "sources {sources:?} do not match role {role}; this is an NBFC sensor-source/configuration limitation, not proof of a physical fan mapping"
                ));
            }
            if let Some(temperature) = temperature
                && temperature.status != RequirementStatus::Satisfied
            {
                reasons.push(format!(
                    "{} temperature is {}: {}",
                    role,
                    temperature.status.label(),
                    temperature.detail
                ));
            }
            let mut validation_unverified = Vec::new();
            if identity.status != RequirementStatus::Satisfied {
                validation_unverified.push("physical fan identity is unverified".to_string());
            }
            if calibration.status != RequirementStatus::Satisfied {
                validation_unverified.push("RPM calibration is unverified".to_string());
            }
            if control.status != RequirementStatus::Satisfied {
                validation_unverified.push("actuator response is unvalidated".to_string());
            }
            FanReadiness {
                index: fan.index,
                name: fan.name.clone(),
                role,
                policy_eligible: reasons.is_empty(),
                mapping_ok,
                sources,
                temperature_status,
                reasons,
                validation_unverified,
            }
        })
        .collect()
}

fn control_mode(status: Option<&NbfcStatus>, safety_state: Option<SafetyState>) -> ControlMode {
    match safety_state {
        Some(SafetyState::FailSafe) => ControlMode::Failsafe,
        Some(SafetyState::Degraded) => ControlMode::Degraded,
        Some(SafetyState::Manual) => ControlMode::Manual,
        Some(SafetyState::SafeAuto) => ControlMode::Automatic,
        None => {
            if status.is_some() {
                ControlMode::Automatic
            } else {
                ControlMode::Unavailable
            }
        }
    }
}

fn next_action(readiness: &ControlReadiness) -> String {
    if !readiness.blockers.is_empty() {
        return format!(
            "Keep automatic control; resolve the blocking requirement(s): {}",
            readiness.blockers.join("; ")
        );
    }
    if readiness.physical_control_validated {
        return "Physical control validation evidence is complete; hardware writes still require the normal policy gate and explicit user confirmation.".to_string();
    }
    "Keep automatic control. Physical identity, RPM calibration and actuator response still require separate external measurement and authorization.".to_string()
}

/// Assess control readiness from supervisor-provided state and explicit
/// validation evidence. The Acer machine must pass all three validation
/// requirements before it can reach `HardwareValidated`.
pub fn assess(input: ReadinessInput<'_>, now: u64) -> ControlReadiness {
    let supervisor = supervisor_requirement(input.supervisor);
    let backend = backend_requirement(input.capabilities, input.status);
    let protected_manual = protected_manual_requirement(input.capabilities);
    let socket = socket_requirement(input.capabilities);
    let cpu = cpu_temperature_requirement(input.snapshot, now, input.config);
    let gpu = gpu_temperature_requirement(input.snapshot, input.status, now, input.config);
    let mapping = fan_mapping_requirement(input.status, input.sensor_config);
    let rpm = rpm_feedback_requirement(input.snapshot, now, input.config);
    let identity = validation_requirement(
        RequirementId::PhysicalIdentity,
        "Physical fan identity",
        input.physical_identity_verified,
        "external tachometer/airflow mapping procedure",
    );
    let calibration = validation_requirement(
        RequirementId::RpmCalibration,
        "RPM calibration",
        input.rpm_calibration_verified,
        "independent RPM reference and calibration record",
    );
    let control = validation_requirement(
        RequirementId::PhysicalControl,
        "Physical control response",
        input.physical_control_validated,
        "controlled actuator-response validation with an independent reference",
    );

    // Global backend trust is required for any manual path, even if one fan's
    // temperature and mapping happen to look usable.
    let essential_control_ok = supervisor.status == RequirementStatus::Satisfied
        && backend.status == RequirementStatus::Satisfied
        && protected_manual.status == RequirementStatus::Satisfied
        && socket.status == RequirementStatus::Satisfied;
    let fans = fan_readiness(
        input.status,
        input.sensor_config,
        &cpu,
        &gpu,
        essential_control_ok,
        &identity,
        &calibration,
        &control,
    );
    let eligible_fans = fans
        .iter()
        .filter(|fan| fan.policy_eligible)
        .map(|fan| fan.index)
        .collect::<Vec<_>>();
    let blocked_fans = fans
        .iter()
        .filter(|fan| !fan.policy_eligible)
        .map(|fan| fan.index)
        .collect::<Vec<_>>();
    let manual_path_eligible = essential_control_ok && !eligible_fans.is_empty();
    let all_fans_eligible = essential_control_ok
        && input.status.is_some_and(|status| !status.fans.is_empty())
        && blocked_fans.is_empty();

    let requirements = vec![
        supervisor,
        backend,
        protected_manual,
        socket,
        cpu,
        gpu,
        mapping,
        rpm,
        identity,
        calibration,
        control,
    ];

    let blockers: Vec<String> = requirements
        .iter()
        .filter(|requirement| {
            requirement.scope == RequirementScope::Control && requirement.status.is_blocking()
        })
        .map(|requirement| format!("{}: {}", requirement.label, requirement.detail))
        .collect();
    let all_control_requirements_satisfied = requirements
        .iter()
        .filter(|requirement| requirement.scope == RequirementScope::Control)
        .all(|requirement| {
            matches!(
                requirement.status,
                RequirementStatus::Satisfied | RequirementStatus::NotApplicable
            )
        });
    let all_fans_control_ready = all_fans_eligible && all_control_requirements_satisfied;
    let validation_missing = requirements
        .iter()
        .filter(|requirement| requirement.scope == RequirementScope::Validation)
        .any(|requirement| requirement.status == RequirementStatus::Missing);
    let validation_unverified = requirements
        .iter()
        .filter(|requirement| requirement.scope == RequirementScope::Validation)
        .any(|requirement| requirement.status == RequirementStatus::Unverified);
    let validation_all_satisfied = requirements
        .iter()
        .filter(|requirement| requirement.scope == RequirementScope::Validation)
        .all(|requirement| requirement.status == RequirementStatus::Satisfied);

    let stage = if !matches!(
        input.supervisor,
        SupervisorState::Healthy | SupervisorState::Connected | SupervisorState::Degraded
    ) {
        ControlStage::Unavailable
    } else if !all_fans_control_ready {
        ControlStage::HardwareDetected
    } else if validation_all_satisfied {
        ControlStage::HardwareValidated
    } else if validation_missing {
        ControlStage::HardwareEligible
    } else if validation_unverified {
        ControlStage::HardwareUnvalidated
    } else {
        ControlStage::HardwareEligible
    };

    let mode = if stage == ControlStage::Unavailable {
        ControlMode::Unavailable
    } else {
        control_mode(input.status, input.safety_state)
    };
    let mut readiness = ControlReadiness {
        schema: "neosense.control_readiness/v1",
        generated_at_ms: now,
        stage,
        mode,
        manual_path_eligible,
        all_fans_eligible: all_fans_control_ready,
        eligible_fans,
        blocked_fans,
        fans,
        physical_identity_verified: input.physical_identity_verified,
        rpm_calibration_verified: input.rpm_calibration_verified,
        physical_control_validated: input.physical_control_validated,
        profile_mode: input.profile_mode,
        requirements,
        blockers,
        next_action: String::new(),
    };
    readiness.next_action = next_action(&readiness);
    readiness
}

/// Convenience wrapper used by diagnostics/CLI. It accepts the same explicit
/// validation flags so callers cannot silently infer them from hardware state.
pub fn assess_now(input: ReadinessInput<'_>) -> ControlReadiness {
    assess(input, now_ms())
}

/// Build readiness from the supervisor-authoritative runtime view. This is the
/// single implementation used by diagnostics, status, the CLI and profiles.
pub fn assess_view(view: &RuntimeView) -> ControlReadiness {
    let now = now_ms();
    let config = SafetyConfig::default();
    let capabilities = view.version.map(|version| {
        view.capabilities
            .clone()
            .unwrap_or_else(|| NbfcCapabilities::from_version(version))
    });
    let status = view.status.as_ref();
    let physical_identity_verified = status
        .and_then(|status| status.selected_config.as_deref())
        .and_then(|name| {
            crate::nbfc_config::identity_report(name, view.sensor_config.as_ref()).ok()
        })
        .is_some_and(|report| {
            !report.fans.is_empty() && report.fans.iter().all(|fan| fan.physical_identity_verified)
        });
    let profile_mode = view
        .supervisor
        .status
        .as_ref()
        .and_then(|status| status.get("profile"))
        .and_then(|profile| profile.get("mode"))
        .and_then(|mode| serde_json::from_value(mode.clone()).ok());
    let safety_state = view
        .supervisor
        .status
        .as_ref()
        .and_then(|status| status.get("safety"))
        .and_then(|safety| safety.get("state"))
        .and_then(|state| serde_json::from_value(state.clone()).ok());
    assess(
        ReadinessInput {
            supervisor: view.supervisor.state,
            snapshot: &view.snapshot,
            capabilities: capabilities.as_ref(),
            status,
            sensor_config: view.sensor_config.as_ref(),
            physical_identity_verified,
            rpm_calibration_verified: false,
            physical_control_validated: false,
            profile_mode,
            safety_state,
            config: &config,
        },
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SensorKind, SensorProvider, SensorSample, Trust, Unit};
    use crate::nbfc::{NbfcFan, NbfcFanSensor, NbfcSocketSecurity, NbfcVersion};

    fn caps(version: &str) -> NbfcCapabilities {
        let version = NbfcVersion::parse(version).unwrap();
        let mut capabilities = NbfcCapabilities::from_version(version);
        capabilities.access_controlled = true;
        capabilities.socket = Some(NbfcSocketSecurity {
            path: "/run/nbfc_service.socket".into(),
            exists: true,
            is_socket: true,
            mode: Some("600".into()),
            uid: Some(0),
            gid: Some(0),
            group_name: Some("root".into()),
            ordinary_group_members: 0,
            group_or_world_writable: false,
            acl_present: false,
            supervisor_uid: 0,
            supervisor_gid: 0,
            supervisor_groups_verified: true,
            identity_source: "current process".into(),
            accessible_to_supervisor: true,
            access_controlled: true,
            error: None,
        });
        capabilities
    }

    fn temp(role: SensorRole, value: f64, trust: Trust, at: u64) -> SensorSample {
        let mut sample = SensorSample::new(
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
        .with_trust(trust);
        sample.sampled_at_ms = at;
        sample
    }

    fn rpm(index: u32, value: f64, at: u64) -> SensorSample {
        let mut sample = SensorSample::new(
            format!("test:rpm:{index}"),
            SensorRole::FanRpm(index),
            SensorKind::Rpm,
            Some(value),
            Unit::Rpm,
            Some(value.to_string()),
            "rpm",
            SensorProvider::Hwmon,
            "acer".into(),
            Some(format!("/sys/acer/fan{}_input", index + 1)),
            None,
            None,
        )
        .with_trust(Trust::Trusted);
        sample.sampled_at_ms = at;
        sample
    }

    fn snapshot(cpu: bool, gpu: bool, rpms: bool, at: u64) -> SensorSnapshot {
        let mut sensors = Vec::new();
        if cpu {
            sensors.push(temp(SensorRole::CpuPackage, 55.0, Trust::Verified, at));
        }
        if gpu {
            sensors.push(temp(SensorRole::GpuDie(0), 55.0, Trust::Verified, at));
        }
        if rpms {
            sensors.push(rpm(0, 4000.0, at));
            sensors.push(rpm(1, 5000.0, at));
        }
        SensorSnapshot {
            sensors,
            errors: Vec::new(),
            collected_at_ms: at,
        }
    }

    fn status() -> NbfcStatus {
        NbfcStatus {
            version: None,
            read_only: Some(false),
            selected_config: Some("Test".into()),
            fans: vec![fan(0, "CPU fan"), fan(1, "GPU fan")],
            warnings: Vec::new(),
        }
    }

    fn fan(index: u32, name: &str) -> NbfcFan {
        NbfcFan {
            index,
            name: name.into(),
            temperature: Some(55.0),
            critical_temperature: Some(95.0),
            auto_mode: Some(true),
            critical: Some(false),
            current_speed: Some(50.0),
            target_speed: Some(50.0),
            requested_speed: Some(50.0),
            speed_steps: Some(100),
        }
    }

    fn sensor_config() -> NbfcSensorConfig {
        NbfcSensorConfig {
            fans: vec![
                NbfcFanSensor {
                    index: 0,
                    name: "CPU fan".into(),
                    sensors: vec!["@CPU".into()],
                    algorithm: Some("Average".into()),
                },
                NbfcFanSensor {
                    index: 1,
                    name: "GPU fan".into(),
                    sensors: vec!["@GPU".into()],
                    algorithm: Some("Average".into()),
                },
            ],
        }
    }

    fn input<'a>(
        snapshot: &'a SensorSnapshot,
        caps: &'a NbfcCapabilities,
        status: &'a NbfcStatus,
        config: &'a NbfcSensorConfig,
    ) -> ReadinessInput<'a> {
        ReadinessInput {
            supervisor: SupervisorState::Healthy,
            snapshot,
            capabilities: Some(caps),
            status: Some(status),
            sensor_config: Some(config),
            physical_identity_verified: false,
            rpm_calibration_verified: false,
            physical_control_validated: false,
            profile_mode: None,
            safety_state: Some(SafetyState::SafeAuto),
            config: Box::leak(Box::new(SafetyConfig::default())),
        }
    }

    #[test]
    fn acer_like_state_is_eligible_but_unvalidated() {
        let now = now_ms();
        let snap = snapshot(true, true, true, now);
        let caps = caps("nbfc 0.5.4");
        let status = status();
        let config = sensor_config();
        let readiness = assess(input(&snap, &caps, &status, &config), now);
        assert_eq!(readiness.stage, ControlStage::HardwareUnvalidated);
        assert!(readiness.manual_path_eligible);
        assert!(!readiness.physical_identity_verified);
        assert!(!readiness.rpm_calibration_verified);
        assert!(!readiness.physical_control_validated);
        assert!(readiness.blockers.is_empty());
        assert_eq!(readiness.fans.len(), 2);
        assert!(readiness.fans.iter().all(|fan| fan.policy_eligible));
        assert!(readiness.fans.iter().all(|fan| {
            fan.validation_unverified
                .iter()
                .any(|r| r.contains("physical"))
        }));
        assert!(readiness.next_action.contains("Physical identity"));
        assert_eq!(
            readiness
                .requirement(RequirementId::RpmFeedback)
                .unwrap()
                .status,
            RequirementStatus::Satisfied
        );
    }

    #[test]
    fn validated_requires_every_validation_requirement() {
        let now = now_ms();
        let snap = snapshot(true, true, true, now);
        let caps = caps("nbfc 0.5.4");
        let status = status();
        let config = sensor_config();
        let mut readiness_input = input(&snap, &caps, &status, &config);
        readiness_input.physical_identity_verified = true;
        readiness_input.rpm_calibration_verified = true;
        readiness_input.physical_control_validated = true;
        let readiness = assess(readiness_input, now);
        assert_eq!(readiness.stage, ControlStage::HardwareValidated);
        assert!(readiness.physical_control_validated);
    }

    #[test]
    fn rpm_feedback_alone_never_validates_control() {
        let now = now_ms();
        let snap = snapshot(false, false, true, now);
        let caps = caps("nbfc 0.5.4");
        let status = status();
        let config = sensor_config();
        let readiness = assess(input(&snap, &caps, &status, &config), now);
        assert_eq!(readiness.stage, ControlStage::HardwareDetected);
        assert!(!readiness.manual_path_eligible);
        assert!(!readiness.physical_control_validated);
        assert!(!readiness.blockers.is_empty());
    }

    #[test]
    fn missing_cpu_temperature_is_a_blocker() {
        let now = now_ms();
        let snap = snapshot(false, true, true, now);
        let caps = caps("nbfc 0.5.4");
        let status = status();
        let config = sensor_config();
        let readiness = assess(input(&snap, &caps, &status, &config), now);
        assert_eq!(
            readiness
                .requirement(RequirementId::CpuTemperature)
                .unwrap()
                .status,
            RequirementStatus::Missing
        );
        // The GPU fan can still be individually eligible when its own
        // temperature and mapping are present.
        assert!(readiness.manual_path_eligible);
        assert!(readiness.blocked_fans.contains(&0));
        assert!(!readiness.fans[0].policy_eligible);
        assert!(
            readiness.fans[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("CPU temperature"))
        );
        assert!(
            readiness
                .blockers
                .iter()
                .any(|b| b.contains("CPU temperature"))
        );
    }

    #[test]
    fn stale_or_untrusted_temperature_fails_not_missing() {
        let now = now_ms();
        let mut snap = snapshot(true, true, true, now);
        snap.sensors[0].sampled_at_ms = now.saturating_sub(10_000);
        let caps = caps("nbfc 0.5.4");
        let status = status();
        let config = sensor_config();
        let readiness = assess(input(&snap, &caps, &status, &config), now);
        assert_eq!(
            readiness
                .requirement(RequirementId::CpuTemperature)
                .unwrap()
                .status,
            RequirementStatus::Failed
        );
    }

    #[test]
    fn gpu_mapping_mismatch_blocks_control() {
        let now = now_ms();
        let snap = snapshot(true, true, true, now);
        let caps = caps("nbfc 0.5.4");
        let status = status();
        let mut config = sensor_config();
        config.fans[1].sensors = vec!["@CPU".into()];
        let readiness = assess(input(&snap, &caps, &status, &config), now);
        assert_eq!(
            readiness
                .requirement(RequirementId::FanMapping)
                .unwrap()
                .status,
            RequirementStatus::Failed
        );
        // CPU fan remains individually eligible; GPU fan is blocked with an
        // explicit source-level reason.
        assert!(readiness.manual_path_eligible);
        assert_eq!(readiness.blocked_fans, vec![1]);
        assert!(readiness.fans[0].policy_eligible);
        assert!(!readiness.fans[1].policy_eligible);
        assert!(
            readiness.fans[1].reasons.iter().any(
                |reason| reason.contains("@CPU") && reason.contains("configuration limitation")
            )
        );
        let json = serde_json::to_value(&readiness).unwrap();
        assert_eq!(json["fans"][1]["policy_eligible"], false);
        assert!(
            json["fans"][1]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason.as_str().unwrap_or("").contains("@CPU"))
        );
    }

    #[test]
    fn insecure_backend_blocks_control() {
        let now = now_ms();
        let snap = snapshot(true, true, true, now);
        let mut caps = caps("nbfc 0.5.4");
        caps.access_controlled = false;
        caps.socket.as_mut().unwrap().access_controlled = false;
        let status = status();
        let config = sensor_config();
        let readiness = assess(input(&snap, &caps, &status, &config), now);
        assert_eq!(readiness.stage, ControlStage::HardwareDetected);
        assert_eq!(
            readiness
                .requirement(RequirementId::ProtectedManual)
                .unwrap()
                .status,
            RequirementStatus::Failed
        );
        assert_eq!(
            readiness
                .requirement(RequirementId::NbfcSocket)
                .unwrap()
                .status,
            RequirementStatus::Failed
        );
    }

    #[test]
    fn no_supervisor_is_unavailable() {
        let now = now_ms();
        let snap = snapshot(true, true, true, now);
        let caps = caps("nbfc 0.5.4");
        let status = status();
        let config = sensor_config();
        let mut readiness_input = input(&snap, &caps, &status, &config);
        readiness_input.supervisor = SupervisorState::NotRunning;
        let readiness = assess(readiness_input, now);
        assert_eq!(readiness.stage, ControlStage::Unavailable);
        assert!(!readiness.manual_path_eligible);
        assert_eq!(readiness.mode, ControlMode::Unavailable);
    }

    #[test]
    fn preview_and_failsafe_are_explicit_not_hardware_validation() {
        let now = now_ms();
        let snap = snapshot(true, true, true, now);
        let caps = caps("nbfc 0.5.4");
        let status = status();
        let config = sensor_config();
        let mut readiness_input = input(&snap, &caps, &status, &config);
        readiness_input.profile_mode = Some(RegulationMode::Preview);
        readiness_input.safety_state = Some(SafetyState::FailSafe);
        let readiness = assess(readiness_input, now);
        assert_eq!(readiness.profile_mode, Some(RegulationMode::Preview));
        assert_eq!(readiness.mode, ControlMode::Failsafe);
        assert_eq!(readiness.stage, ControlStage::HardwareUnvalidated);
        assert!(!readiness.physical_control_validated);
    }
}
