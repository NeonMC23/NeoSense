//! Supervisor-owned physical fan-validation state and evidence.
//!
//! This module is deliberately separate from ordinary manual control. A
//! validation session is the only place where the validation-only 0% step is
//! permitted. The normal manual floor in [`crate::policy::SafetyConfig`] stays
//! unchanged.
//!
//! The state machine is pure. The supervisor owns timing, writes, recovery and
//! evidence persistence; the CLI only authorizes and records the operator's
//! observation.

use crate::model::{SensorProvider, SensorSnapshot, SensorStatus, Trust};
use crate::nbfc::{NbfcStatus, NbfcVersion};
use crate::policy::FanRole;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const RECORD_SCHEMA: &str = "neosense.validation_record/v1";
pub const SESSION_SCHEMA: &str = "neosense.validation_session/v1";
pub const PREFLIGHT_SCHEMA: &str = "neosense.validation_preflight/v1";
pub const STEP_DURATION_MS: u64 = 5_000;
pub const STEP_WATCHDOG_MS: u64 = STEP_DURATION_MS + 1_000;
pub const SESSION_TTL_MS: u64 = 120_000;
pub const CPU_MAX_TEMP_C: f64 = 85.0;
pub const GPU_MAX_TEMP_C: f64 = 80.0;
pub const MAX_EVIDENCE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_HISTORY_RECORDS: usize = 2_000;

/// Conservative, validation-specific limits. These are separate from the
/// ordinary manual-control defaults so the latter cannot be weakened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationLimits {
    pub step_duration_ms: u64,
    /// Watchdog deadline. It may exceed the requested hold by a small margin so
    /// verification can complete before a timeout abort.
    pub step_timeout_ms: u64,
    pub session_ttl_ms: u64,
    pub cpu_max_temp_c: f64,
    pub gpu_max_temp_c: f64,
    /// The validation-only 0% step. This does not affect ordinary manual control.
    pub allow_zero_percent: bool,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            step_duration_ms: STEP_DURATION_MS,
            step_timeout_ms: STEP_WATCHDOG_MS,
            session_ttl_ms: SESSION_TTL_MS,
            cpu_max_temp_c: CPU_MAX_TEMP_C,
            gpu_max_temp_c: GPU_MAX_TEMP_C,
            allow_zero_percent: true,
        }
    }
}

impl ValidationLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.step_duration_ms == 0 || self.step_duration_ms > STEP_DURATION_MS {
            return Err(format!(
                "validation step duration must be 1-{STEP_DURATION_MS} ms"
            ));
        }
        if self.step_timeout_ms < self.step_duration_ms || self.step_timeout_ms > 10_000 {
            return Err(
                "validation step timeout must be between the step duration and 10000 ms".into(),
            );
        }
        if !(10_000..=600_000).contains(&self.session_ttl_ms) {
            return Err("validation session TTL must be 10000-600000 ms".into());
        }
        if !(40.0..=95.0).contains(&self.cpu_max_temp_c) {
            return Err("validation CPU ceiling must be 40-95 °C".into());
        }
        if !(40.0..=95.0).contains(&self.gpu_max_temp_c) {
            return Err("validation GPU ceiling must be 40-95 °C".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedSide {
    Left,
    Right,
}

impl ExpectedSide {
    pub fn for_role(role: FanRole) -> Option<Self> {
        match role {
            FanRole::Cpu => Some(Self::Left),
            FanRole::Gpu => Some(Self::Right),
            FanRole::Unknown => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "LEFT",
            Self::Right => "RIGHT",
        }
    }
}

impl std::fmt::Display for ExpectedSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStep {
    Zero,
    Fifty,
    Hundred,
}

impl ValidationStep {
    pub const SEQUENCE: [Self; 3] = [Self::Zero, Self::Fifty, Self::Hundred];

    pub fn percent(self) -> f64 {
        match self {
            Self::Zero => 0.0,
            Self::Fifty => 50.0,
            Self::Hundred => 100.0,
        }
    }

    pub fn number(self) -> u8 {
        match self {
            Self::Zero => 1,
            Self::Fifty => 2,
            Self::Hundred => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Zero => "0%",
            Self::Fifty => "50%",
            Self::Hundred => "100%",
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::Zero => Some(Self::Fifty),
            Self::Fifty => Some(Self::Hundred),
            Self::Hundred => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationState {
    /// The session exists but no step is currently active.
    Idle,
    RunningStep,
    Recovering,
    AwaitingObservation,
    Completed,
    Rejected,
    Aborted,
    Failed,
}

impl ValidationState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Rejected | Self::Aborted | Self::Failed
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::RunningStep => "running step",
            Self::Recovering => "recovering",
            Self::AwaitingObservation => "awaiting observation",
            Self::Completed => "completed",
            Self::Rejected => "rejected by operator",
            Self::Aborted => "aborted",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationEvent {
    Preflight,
    PreflightFailed,
    Authorized,
    StepStarted,
    StepVerified,
    StepFailed,
    RecoveryStarted,
    RecoveryCompleted,
    ObservationRecorded,
    ContinuationAuthorized,
    SessionAborted,
    SessionExpired,
    ExternalControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationEvidenceSource {
    OperatorAttestation,
    BackendEcho,
    ProvisionalEc,
    ExternalInstrument,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    PreflightFailed,
    StepTimeout,
    VerificationFailed,
    SafetyGuard(String),
    OperatorRejected,
    ClientAborted,
    SessionExpired,
    ExternalControl,
    RecoveryFailed(String),
    SequenceFailed(String),
    SupervisorShutdown,
    EvidenceWriteFailed(String),
}

impl StopReason {
    pub fn label(&self) -> String {
        match self {
            Self::PreflightFailed => "preflight failed".to_string(),
            Self::StepTimeout => "step timeout".to_string(),
            Self::VerificationFailed => "step verification failed".to_string(),
            Self::SafetyGuard(reason) => format!("safety guard: {reason}"),
            Self::OperatorRejected => "operator rejected the physical observation".to_string(),
            Self::ClientAborted => "operator aborted the session".to_string(),
            Self::SessionExpired => "validation session expired".to_string(),
            Self::ExternalControl => "external fan control changed the session".to_string(),
            Self::RecoveryFailed(reason) => format!("automatic recovery failed: {reason}"),
            Self::SequenceFailed(reason) => format!("validation sequence failed: {reason}"),
            Self::SupervisorShutdown => "supervisor shutdown".to_string(),
            Self::EvidenceWriteFailed(reason) => {
                format!("validation evidence write failed: {reason}")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationFan {
    pub index: u32,
    pub backend_id: String,
    pub name: String,
    pub role: FanRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorAuthorization {
    pub authorized: bool,
    pub authorized_at_ms: u64,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationAnswer {
    Yes,
    No,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorObservation {
    pub answer: ObservationAnswer,
    pub observed_side: Option<ExpectedSide>,
    pub expected_side: ExpectedSide,
    pub note: Option<String>,
    pub recorded_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemperatureEvidence {
    pub value_c: f64,
    pub sampled_at_ms: u64,
    pub age_ms: u64,
    pub trust: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanStateEvidence {
    pub index: u32,
    pub name: String,
    pub auto_mode: Option<bool>,
    pub target_percent: Option<f64>,
    pub reported_percent: Option<f64>,
    pub critical: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EcRawEvidence {
    pub role: String,
    pub value: Option<f64>,
    pub raw_value: Option<String>,
    pub unit: String,
    pub trust: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationTelemetry {
    pub collected_at_ms: u64,
    pub cpu: Option<TemperatureEvidence>,
    pub gpu: Option<TemperatureEvidence>,
    pub nbfc_fans: Vec<FanStateEvidence>,
    pub ec_values: Vec<EcRawEvidence>,
}

impl ValidationTelemetry {
    pub fn from_state(snapshot: &SensorSnapshot, status: Option<&NbfcStatus>, now: u64) -> Self {
        let cpu = snapshot
            .cpu_package(now, 3_000)
            .map(|sample| TemperatureEvidence {
                value_c: sample.value.unwrap_or_default(),
                sampled_at_ms: sample.sampled_at_ms,
                age_ms: sample.age_ms(now),
                trust: sample.health.trust.short_label().to_string(),
                status: format!("{:?}", sample.status).to_ascii_lowercase(),
            });
        let gpu = snapshot
            .gpu_indexes()
            .into_iter()
            .find_map(|index| snapshot.gpu_die(index, now, 3_000))
            .map(|sample| TemperatureEvidence {
                value_c: sample.value.unwrap_or_default(),
                sampled_at_ms: sample.sampled_at_ms,
                age_ms: sample.age_ms(now),
                trust: sample.health.trust.short_label().to_string(),
                status: format!("{:?}", sample.status).to_ascii_lowercase(),
            });
        let nbfc_fans = status
            .map(|status| {
                status
                    .fans
                    .iter()
                    .map(|fan| FanStateEvidence {
                        index: fan.index,
                        name: fan.name.clone(),
                        auto_mode: fan.auto_mode,
                        target_percent: fan.target_speed,
                        reported_percent: fan.current_speed,
                        critical: fan.critical,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ec_values = snapshot
            .sensors
            .iter()
            .filter(|sample| sample.provider == SensorProvider::Ec)
            .map(|sample| EcRawEvidence {
                role: sample.role.to_string(),
                value: sample.value,
                raw_value: sample.raw_value.clone(),
                unit: format!("{:?}", sample.unit).to_ascii_lowercase(),
                trust: sample.health.trust.short_label().to_string(),
                status: format!("{:?}", sample.status).to_ascii_lowercase(),
            })
            .collect();
        Self {
            collected_at_ms: snapshot.collected_at_ms,
            cpu,
            gpu,
            nbfc_fans,
            ec_values,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryEvidence {
    pub attempted: bool,
    pub backend_auto: bool,
    pub ec_verified: Option<bool>,
    pub detail: String,
    pub completed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRecord {
    pub step: ValidationStep,
    pub requested_percent: f64,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    pub backend_target_percent: Option<f64>,
    pub backend_reported_percent: Option<f64>,
    pub backend_mode: Option<String>,
    pub verified: bool,
    #[serde(default)]
    pub command_sent_at_ms: Option<u64>,
    #[serde(default)]
    pub backend_response_at_ms: Option<u64>,
    #[serde(default)]
    pub command_accepted: bool,
    #[serde(default)]
    pub backend_target_verified: bool,
    #[serde(default)]
    pub backend_mode_verified: bool,
    #[serde(default)]
    pub readback_plausible: Option<bool>,
    #[serde(default)]
    pub hold_started_at_ms: Option<u64>,
    #[serde(default)]
    pub hold_finished_at_ms: Option<u64>,
    #[serde(default)]
    pub configured_hold_ms: Option<u64>,
    #[serde(default)]
    pub actual_hold_ms: Option<u64>,
    pub telemetry: ValidationTelemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationSession {
    pub schema: String,
    pub id: String,
    pub fan: ValidationFan,
    pub expected_side: ExpectedSide,
    pub state: ValidationState,
    pub created_at_ms: u64,
    pub deadline_ms: u64,
    pub authorized: OperatorAuthorization,
    pub recovery_armed: bool,
    pub limits: ValidationLimits,
    pub next_step: Option<ValidationStep>,
    pub active_step: Option<ValidationStep>,
    pub step_started_at_ms: Option<u64>,
    pub step_deadline_ms: Option<u64>,
    #[serde(default)]
    pub write_pending: bool,
    #[serde(default)]
    pub command_sent_at_ms: Option<u64>,
    #[serde(default)]
    pub backend_response_at_ms: Option<u64>,
    pub completed_steps: Vec<StepRecord>,
    pub recovery: Option<RecoveryEvidence>,
    pub observation: Option<OperatorObservation>,
    pub stop_reason: Option<StopReason>,
    pub selected_config: Option<String>,
    pub nbfc_version: Option<String>,
}

impl ValidationSession {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        fan: ValidationFan,
        expected_side: ExpectedSide,
        limits: ValidationLimits,
        now: u64,
        selected_config: Option<String>,
        nbfc_version: Option<String>,
    ) -> Self {
        Self {
            schema: SESSION_SCHEMA.to_string(),
            id,
            fan,
            expected_side,
            state: ValidationState::Idle,
            created_at_ms: now,
            deadline_ms: now + limits.session_ttl_ms,
            authorized: OperatorAuthorization {
                authorized: true,
                authorized_at_ms: now,
                source: "cli_explicit_yes".to_string(),
            },
            recovery_armed: true,
            limits,
            next_step: Some(ValidationStep::Zero),
            active_step: None,
            step_started_at_ms: None,
            step_deadline_ms: None,
            write_pending: false,
            command_sent_at_ms: None,
            backend_response_at_ms: None,
            completed_steps: Vec::new(),
            recovery: None,
            observation: None,
            stop_reason: None,
            selected_config,
            nbfc_version,
        }
    }

    pub fn begin_step(&mut self, step: ValidationStep, now: u64) -> Result<(), StopReason> {
        if self.state != ValidationState::Idle && self.state != ValidationState::RunningStep {
            return Err(StopReason::SequenceFailed(format!(
                "cannot start a step while state is {}",
                self.state.label()
            )));
        }
        if self.next_step != Some(step) {
            return Err(StopReason::SequenceFailed(format!(
                "expected next step {:?}, got {:?}",
                self.next_step, step
            )));
        }
        if self.active_step.is_some() {
            return Err(StopReason::SequenceFailed(
                "a previous step is still active".to_string(),
            ));
        }
        self.state = ValidationState::RunningStep;
        self.active_step = Some(step);
        self.write_pending = true;
        self.command_sent_at_ms = None;
        self.backend_response_at_ms = None;
        self.step_started_at_ms = Some(now);
        self.step_deadline_ms = Some(now + self.limits.step_duration_ms);
        Ok(())
    }

    pub fn finish_step(
        &mut self,
        step: ValidationStep,
        record: StepRecord,
    ) -> Result<(), StopReason> {
        if self.state != ValidationState::RunningStep || self.active_step != Some(step) {
            return Err(StopReason::SequenceFailed(format!(
                "cannot finish step {step:?} in state {}",
                self.state.label()
            )));
        }
        self.completed_steps.push(record);
        self.active_step = None;
        self.step_started_at_ms = None;
        self.step_deadline_ms = None;
        self.write_pending = false;
        self.command_sent_at_ms = None;
        self.backend_response_at_ms = None;
        self.next_step = step.next();
        Ok(())
    }

    pub fn mark_recovering(&mut self) -> Result<(), StopReason> {
        if self.state != ValidationState::RunningStep || self.next_step.is_some() {
            return Err(StopReason::SequenceFailed(
                "recovery requested before the sequence completed".to_string(),
            ));
        }
        self.active_step = None;
        self.step_started_at_ms = None;
        self.step_deadline_ms = None;
        self.state = ValidationState::Recovering;
        Ok(())
    }

    pub fn mark_awaiting_observation(
        &mut self,
        recovery: RecoveryEvidence,
    ) -> Result<(), StopReason> {
        if self.state != ValidationState::Recovering {
            return Err(StopReason::SequenceFailed(
                "observation requested before recovery".to_string(),
            ));
        }
        self.recovery = Some(recovery);
        self.state = ValidationState::AwaitingObservation;
        Ok(())
    }

    pub fn record_observation(
        &mut self,
        answer: ObservationAnswer,
        note: Option<String>,
        now: u64,
    ) -> Result<(), StopReason> {
        if self.state != ValidationState::AwaitingObservation {
            return Err(StopReason::SequenceFailed(
                "observation is only accepted after verified recovery".to_string(),
            ));
        }
        let observed_side = match answer {
            ObservationAnswer::Yes => Some(self.expected_side),
            ObservationAnswer::No => None,
        };
        self.observation = Some(OperatorObservation {
            answer,
            observed_side,
            expected_side: self.expected_side,
            note,
            recorded_at_ms: now,
        });
        self.state = match answer {
            ObservationAnswer::Yes => ValidationState::Completed,
            ObservationAnswer::No => ValidationState::Rejected,
        };
        if answer == ObservationAnswer::No {
            self.stop_reason = Some(StopReason::OperatorRejected);
        }
        Ok(())
    }

    pub fn abort(&mut self, reason: StopReason) -> bool {
        if self.state.is_terminal() {
            return false;
        }
        self.state = ValidationState::Aborted;
        self.stop_reason = Some(reason);
        self.active_step = None;
        self.step_started_at_ms = None;
        self.step_deadline_ms = None;
        true
    }

    pub fn fail(&mut self, reason: StopReason) -> bool {
        if self.state.is_terminal() {
            return false;
        }
        self.state = ValidationState::Failed;
        self.stop_reason = Some(reason);
        self.active_step = None;
        self.step_started_at_ms = None;
        self.step_deadline_ms = None;
        true
    }

    pub fn remaining_step_ms(&self, now: u64) -> u64 {
        self.step_deadline_ms
            .map(|deadline| deadline.saturating_sub(now))
            .unwrap_or(0)
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationStepSpec {
    pub step: ValidationStep,
    pub requested_percent: f64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationPreflight {
    pub schema: String,
    pub ready: bool,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub fan: ValidationFan,
    pub expected_side: ExpectedSide,
    pub other_fan: Option<ValidationFan>,
    pub other_fan_auto: bool,
    pub steps: Vec<ValidationStepSpec>,
    pub limits: ValidationLimits,
    pub telemetry: ValidationTelemetry,
    pub safety_state: Option<String>,
    pub selected_config: Option<String>,
    pub nbfc_version: Option<String>,
}

impl ValidationPreflight {
    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "fan {} ({}) · logical role {} · expected side {} (hypothesis)",
                self.fan.index, self.fan.name, self.fan.role, self.expected_side
            ),
            format!(
                "other fan: {}",
                self.other_fan
                    .as_ref()
                    .map(|fan| format!(
                        "{} ({}) · {}",
                        fan.index,
                        fan.name,
                        if self.other_fan_auto {
                            "automatic"
                        } else {
                            "NOT automatic"
                        }
                    ))
                    .unwrap_or_else(|| "not discovered".to_string())
            ),
            format!(
                "sequence: 0% / 50% / 100%, {} ms per step, then automatic recovery",
                self.limits.step_duration_ms
            ),
            format!(
                "temperature ceilings: CPU {:.0} °C, GPU {:.0} °C",
                self.limits.cpu_max_temp_c, self.limits.gpu_max_temp_c
            ),
        ];
        if let Some(cpu) = &self.telemetry.cpu {
            lines.push(format!(
                "CPU temperature: {:.1} °C (age {} ms, {}, {})",
                cpu.value_c, cpu.age_ms, cpu.trust, cpu.status
            ));
        } else {
            lines.push("CPU temperature: unavailable".to_string());
        }
        if let Some(gpu) = &self.telemetry.gpu {
            lines.push(format!(
                "GPU temperature: {:.1} °C (age {} ms, {}, {})",
                gpu.value_c, gpu.age_ms, gpu.trust, gpu.status
            ));
        } else {
            lines.push("GPU temperature: unavailable".to_string());
        }
        if self.ready {
            lines.push("readiness: READY (control safety preflight passed)".to_string());
        } else {
            for blocker in &self.blockers {
                lines.push(format!("blocker: {blocker}"));
            }
        }
        for warning in &self.warnings {
            lines.push(format!("warning: {warning}"));
        }
        lines
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRecord {
    pub schema: String,
    pub session_id: String,
    pub timestamp_ms: u64,
    pub event: ValidationEvent,
    pub session_state: String,
    pub fan: Option<ValidationFan>,
    pub expected_side: Option<ExpectedSide>,
    pub observed_side: Option<ExpectedSide>,
    pub step: Option<ValidationStep>,
    pub requested_percent: Option<f64>,
    pub step_duration_ms: Option<u64>,
    pub command_sent_at_ms: Option<u64>,
    pub backend_response_at_ms: Option<u64>,
    pub backend_target_percent: Option<f64>,
    pub backend_reported_percent: Option<f64>,
    pub backend_mode: Option<String>,
    /// Deprecated compatibility field. It means backend target and mode only.
    #[serde(default)]
    pub verification: Option<bool>,
    #[serde(default)]
    pub command_accepted: Option<bool>,
    #[serde(default)]
    pub backend_target_verified: Option<bool>,
    #[serde(default)]
    pub backend_mode_verified: Option<bool>,
    #[serde(default)]
    pub readback_percent: Option<f64>,
    #[serde(default)]
    pub readback_plausible: Option<bool>,
    #[serde(default)]
    pub physical_response_verified: bool,
    #[serde(default)]
    pub verification_scope: Option<String>,
    #[serde(default)]
    pub hold_started_at_ms: Option<u64>,
    #[serde(default)]
    pub hold_finished_at_ms: Option<u64>,
    #[serde(default)]
    pub configured_hold_ms: Option<u64>,
    #[serde(default)]
    pub actual_hold_ms: Option<u64>,
    pub automatic_mode_verified: Option<bool>,
    pub automatic_mode_evidence: Vec<String>,
    pub temperatures: Option<ValidationTelemetry>,
    pub safety_state: Option<String>,
    pub abort_reasons: Vec<String>,
    pub operator_authorization: Option<OperatorAuthorization>,
    pub recovery_armed: bool,
    pub operator_observation: Option<OperatorObservation>,
    pub operator_notes: Option<String>,
    pub recovery: Option<RecoveryEvidence>,
    pub supervisor_version: String,
    pub selected_config: Option<String>,
    pub evidence_sources: Vec<ValidationEvidenceSource>,
}

impl ValidationRecord {
    pub fn for_session(
        session: &ValidationSession,
        event: ValidationEvent,
        timestamp_ms: u64,
        telemetry: Option<ValidationTelemetry>,
        safety_state: Option<String>,
    ) -> Self {
        let last = session.completed_steps.last();
        let command_sent_at_ms = session
            .command_sent_at_ms
            .or_else(|| last.and_then(|step| step.command_sent_at_ms));
        let backend_response_at_ms = session
            .backend_response_at_ms
            .or_else(|| last.and_then(|step| step.backend_response_at_ms));
        let backend_target_percent = last.and_then(|step| step.backend_target_percent);
        let backend_reported_percent = last.and_then(|step| step.backend_reported_percent);
        let backend_mode = last.and_then(|step| step.backend_mode.clone());
        let backend_target_verified = last.map(|step| step.backend_target_verified);
        let backend_mode_verified = last.map(|step| step.backend_mode_verified);
        let readback_plausible = last.and_then(|step| step.readback_plausible);
        let hold_started_at_ms = last.and_then(|step| step.hold_started_at_ms);
        let hold_finished_at_ms = last.and_then(|step| step.hold_finished_at_ms);
        let configured_hold_ms = last.and_then(|step| step.configured_hold_ms);
        let actual_hold_ms = last.and_then(|step| step.actual_hold_ms);
        Self {
            schema: RECORD_SCHEMA.to_string(),
            session_id: session.id.clone(),
            timestamp_ms,
            event,
            session_state: session.state.label().to_string(),
            fan: Some(session.fan.clone()),
            expected_side: Some(session.expected_side),
            observed_side: session
                .observation
                .as_ref()
                .and_then(|observation| observation.observed_side),
            step: session
                .active_step
                .or_else(|| session.completed_steps.last().map(|record| record.step)),
            requested_percent: session.active_step.map(ValidationStep::percent),
            step_duration_ms: session.active_step.map(|_| session.limits.step_duration_ms),
            command_sent_at_ms,
            backend_response_at_ms,
            backend_target_percent,
            backend_reported_percent,
            backend_mode,
            verification: None,
            command_accepted: last.map(|step| step.command_accepted),
            backend_target_verified,
            backend_mode_verified,
            readback_percent: backend_reported_percent,
            readback_plausible,
            physical_response_verified: false,
            verification_scope: None,
            hold_started_at_ms,
            hold_finished_at_ms,
            configured_hold_ms,
            actual_hold_ms,
            automatic_mode_verified: session
                .recovery
                .as_ref()
                .and_then(|recovery| recovery.ec_verified),
            automatic_mode_evidence: session
                .recovery
                .as_ref()
                .map(|recovery| vec![recovery.detail.clone()])
                .unwrap_or_default(),
            temperatures: telemetry.clone(),
            safety_state,
            abort_reasons: session
                .stop_reason
                .as_ref()
                .map(|reason| vec![reason.label()])
                .unwrap_or_default(),
            operator_authorization: Some(session.authorized.clone()),
            recovery_armed: session.recovery_armed,
            operator_observation: session.observation.clone(),
            operator_notes: session
                .observation
                .as_ref()
                .and_then(|observation| observation.note.clone()),
            recovery: session.recovery.clone(),
            supervisor_version: env!("CARGO_PKG_VERSION").to_string(),
            selected_config: session.selected_config.clone(),
            evidence_sources: {
                let mut sources = Vec::new();
                if matches!(
                    event,
                    ValidationEvent::StepStarted
                        | ValidationEvent::StepVerified
                        | ValidationEvent::StepFailed
                        | ValidationEvent::RecoveryStarted
                        | ValidationEvent::RecoveryCompleted
                ) {
                    sources.push(ValidationEvidenceSource::BackendEcho);
                }
                if telemetry
                    .as_ref()
                    .is_some_and(|telemetry| !telemetry.ec_values.is_empty())
                {
                    sources.push(ValidationEvidenceSource::ProvisionalEc);
                }
                if event == ValidationEvent::ObservationRecorded && session.observation.is_some() {
                    sources.push(ValidationEvidenceSource::OperatorAttestation);
                }
                sources
            },
        }
    }
}

/// Append-only JSONL evidence store, owned by the supervisor.
#[derive(Debug, Clone)]
pub struct EvidenceStore {
    dir: PathBuf,
}

impl EvidenceStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn prepare(&self) -> Result<(), String> {
        fs::create_dir_all(&self.dir)
            .map_err(|error| format!("create {}: {error}", self.dir.display()))?;
        // Verify a private file can be created without leaving a record.
        let probe = self.dir.join(".write-probe");
        let result = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&probe)
            .and_then(|mut file| file.write_all(b"ok\n"));
        let _ = fs::remove_file(&probe);
        result.map_err(|error| format!("write {}: {error}", self.dir.display()))
    }

    fn path_for(&self, session_id: &str) -> Result<PathBuf, String> {
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err("invalid validation session id".to_string());
        }
        Ok(self.dir.join(format!("{session_id}.jsonl")))
    }

    pub fn append(&self, record: &ValidationRecord) -> Result<(), String> {
        self.prepare()?;
        let path = self.path_for(&record.session_id)?;
        let mut line = serde_json::to_string(record)
            .map_err(|error| format!("serialize validation record: {error}"))?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        file.write_all(line.as_bytes())
            .map_err(|error| format!("append {}: {error}", path.display()))?;
        file.sync_data()
            .map_err(|error| format!("sync {}: {error}", path.display()))?;
        Ok(())
    }

    pub fn read_with_warnings(&self, session_id: Option<&str>) -> Result<EvidenceRead, String> {
        let files = match session_id {
            Some(id) => vec![self.path_for(id)?],
            None => {
                let entries = match fs::read_dir(&self.dir) {
                    Ok(entries) => entries,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(EvidenceRead {
                            records: Vec::new(),
                            warnings: Vec::new(),
                        });
                    }
                    Err(error) => {
                        return Err(format!("read {}: {error}", self.dir.display()));
                    }
                };
                let mut files = Vec::new();
                for entry in entries {
                    let entry = entry.map_err(|error| format!("read dir entry: {error}"))?;
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "jsonl")
                    {
                        files.push(path);
                    }
                }
                files.sort();
                files
            }
        };
        let mut records = Vec::new();
        let mut warnings = Vec::new();
        for path in files {
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("stat {}: {error}", path.display())),
            };
            if metadata.len() > MAX_EVIDENCE_BYTES {
                return Err(format!(
                    "validation evidence file {} is too large",
                    path.display()
                ));
            }
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("read {}: {error}", path.display()))?;
            let non_empty: Vec<&str> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            for (index, line) in non_empty.iter().enumerate() {
                match serde_json::from_str::<ValidationRecord>(line) {
                    Ok(record) => records.push(record),
                    Err(error) if index + 1 == non_empty.len() => {
                        warnings.push(format!(
                            "ignored incomplete final validation record in {}: {error}",
                            path.display()
                        ));
                        break;
                    }
                    Err(error) => {
                        return Err(format!("parse {}: {error}", path.display()));
                    }
                }
                if records.len() >= MAX_HISTORY_RECORDS {
                    warnings.push(format!(
                        "validation history limited to {MAX_HISTORY_RECORDS} records"
                    ));
                    return Ok(EvidenceRead { records, warnings });
                }
            }
        }
        Ok(EvidenceRead { records, warnings })
    }

    pub fn read(&self, session_id: Option<&str>) -> Result<Vec<ValidationRecord>, String> {
        Ok(self.read_with_warnings(session_id)?.records)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceRead {
    pub records: Vec<ValidationRecord>,
    pub warnings: Vec<String>,
}

/// Public export helper used by the read-only history command.
pub fn record_schema() -> &'static str {
    RECORD_SCHEMA
}

pub fn nbfc_version_string(version: Option<NbfcVersion>) -> Option<String> {
    version.map(|version| version.to_string())
}

/// Whether a sample is an acceptable validation telemetry input.
pub fn sample_is_usable(sample: &crate::model::SensorSample) -> bool {
    sample.status == SensorStatus::Ok
        && sample.value.is_some()
        && sample.health.trust.is_at_least(Trust::Trusted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fan(role: FanRole) -> ValidationFan {
        ValidationFan {
            index: if role == FanRole::Cpu { 0 } else { 1 },
            backend_id: format!("nbfc:{}", if role == FanRole::Cpu { 0 } else { 1 }),
            name: format!("{role} fan"),
            role,
        }
    }

    fn session() -> ValidationSession {
        ValidationSession::new(
            "session-1".into(),
            fan(FanRole::Cpu),
            ExpectedSide::Left,
            ValidationLimits {
                step_duration_ms: 10,
                step_timeout_ms: 20,
                session_ttl_ms: 10_000,
                ..ValidationLimits::default()
            },
            now(),
            Some("test".into()),
            Some("0.5.4".into()),
        )
    }

    fn step_record(step: ValidationStep) -> StepRecord {
        StepRecord {
            step,
            requested_percent: step.percent(),
            started_at_ms: 1,
            finished_at_ms: 2,
            backend_target_percent: Some(step.percent()),
            backend_reported_percent: None,
            backend_mode: Some("manual".into()),
            verified: true,
            command_sent_at_ms: None,
            backend_response_at_ms: None,
            command_accepted: true,
            backend_target_verified: true,
            backend_mode_verified: true,
            readback_plausible: None,
            hold_started_at_ms: Some(1),
            hold_finished_at_ms: Some(2),
            configured_hold_ms: Some(10),
            actual_hold_ms: Some(1),
            telemetry: ValidationTelemetry {
                collected_at_ms: 2,
                cpu: None,
                gpu: None,
                nbfc_fans: Vec::new(),
                ec_values: Vec::new(),
            },
        }
    }

    fn now() -> u64 {
        crate::model::now_ms()
    }

    #[test]
    fn authorization_is_required_and_steps_are_ordered() {
        let mut session = session();
        assert_eq!(session.next_step, Some(ValidationStep::Zero));
        assert!(session.begin_step(ValidationStep::Fifty, now()).is_err());
        assert!(session.begin_step(ValidationStep::Zero, now()).is_ok());
        assert!(
            session
                .finish_step(ValidationStep::Zero, step_record(ValidationStep::Zero))
                .is_ok()
        );
        assert!(session.begin_step(ValidationStep::Hundred, now()).is_err());
        assert!(session.begin_step(ValidationStep::Fifty, now()).is_ok());
    }

    #[test]
    fn observation_is_impossible_before_recovery() {
        let mut session = session();
        assert!(
            session
                .record_observation(ObservationAnswer::Yes, None, now())
                .is_err()
        );
        session.begin_step(ValidationStep::Zero, now()).unwrap();
        session
            .finish_step(ValidationStep::Zero, step_record(ValidationStep::Zero))
            .unwrap();
        session.begin_step(ValidationStep::Fifty, now()).unwrap();
        session
            .finish_step(ValidationStep::Fifty, step_record(ValidationStep::Fifty))
            .unwrap();
        session.begin_step(ValidationStep::Hundred, now()).unwrap();
        session
            .finish_step(
                ValidationStep::Hundred,
                step_record(ValidationStep::Hundred),
            )
            .unwrap();
        assert!(session.mark_awaiting_observation(recovery()).is_err());
        session.mark_recovering().unwrap();
        session.mark_awaiting_observation(recovery()).unwrap();
        session
            .record_observation(ObservationAnswer::Yes, Some("left changed".into()), now())
            .unwrap();
        assert_eq!(session.state, ValidationState::Completed);
        assert_eq!(
            session.observation.unwrap().observed_side,
            Some(ExpectedSide::Left)
        );
    }

    fn recovery() -> RecoveryEvidence {
        RecoveryEvidence {
            attempted: true,
            backend_auto: true,
            ec_verified: Some(false),
            detail: "backend automatic; EC unverified".into(),
            completed_at_ms: now(),
        }
    }

    #[test]
    fn no_records_rejection_and_never_advances() {
        let mut session = session();
        session.begin_step(ValidationStep::Zero, now()).unwrap();
        session
            .finish_step(ValidationStep::Zero, step_record(ValidationStep::Zero))
            .unwrap();
        session.begin_step(ValidationStep::Fifty, now()).unwrap();
        session
            .finish_step(ValidationStep::Fifty, step_record(ValidationStep::Fifty))
            .unwrap();
        session.begin_step(ValidationStep::Hundred, now()).unwrap();
        session
            .finish_step(
                ValidationStep::Hundred,
                step_record(ValidationStep::Hundred),
            )
            .unwrap();
        session.mark_recovering().unwrap();
        session.mark_awaiting_observation(recovery()).unwrap();
        session
            .record_observation(ObservationAnswer::No, None, now())
            .unwrap();
        assert_eq!(session.state, ValidationState::Rejected);
        assert_eq!(session.stop_reason, Some(StopReason::OperatorRejected));
        assert_eq!(session.next_step, None);
        assert!(
            session
                .record_observation(ObservationAnswer::Yes, None, now())
                .is_err()
        );
    }

    #[test]
    fn abort_is_terminal_and_idempotent() {
        let mut session = session();
        session.begin_step(ValidationStep::Zero, now()).unwrap();
        assert!(session.abort(StopReason::ClientAborted));
        assert!(!session.abort(StopReason::ClientAborted));
        assert_eq!(session.state, ValidationState::Aborted);
        assert!(session.is_terminal());
    }

    #[test]
    fn side_assumption_is_explicit_and_matches_role() {
        assert_eq!(
            ExpectedSide::for_role(FanRole::Cpu),
            Some(ExpectedSide::Left)
        );
        assert_eq!(
            ExpectedSide::for_role(FanRole::Gpu),
            Some(ExpectedSide::Right)
        );
        assert_eq!(ExpectedSide::for_role(FanRole::Unknown), None);
        assert_eq!(
            ValidationStep::SEQUENCE.map(ValidationStep::percent),
            [0.0, 50.0, 100.0]
        );
    }

    #[test]
    fn validation_limits_are_stricter_than_ordinary_manual_control() {
        let limits = ValidationLimits::default();
        assert_eq!(limits.step_duration_ms, 5_000);
        assert!(limits.step_timeout_ms > limits.step_duration_ms);
        assert!(limits.allow_zero_percent);
        limits.validate().unwrap();
        let normal = crate::policy::SafetyConfig::default();
        assert_eq!(normal.min_percent, 20);
    }

    #[test]
    fn evidence_store_is_append_only_and_validates_schema() {
        let dir = tempfile::tempdir().unwrap();
        let store = EvidenceStore::new(dir.path());
        let session = session();
        let record = ValidationRecord::for_session(
            &session,
            ValidationEvent::Authorized,
            now(),
            None,
            Some("safe_auto".into()),
        );
        store.append(&record).unwrap();
        store.append(&record).unwrap();
        let records = store.read(Some(&session.id)).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].schema, RECORD_SCHEMA);
        assert_eq!(records[0].session_id, "session-1");
        assert!(records[0].evidence_sources.is_empty());
        assert!(
            records[0]
                .operator_authorization
                .as_ref()
                .unwrap()
                .authorized
        );
    }

    #[test]
    fn telemetry_keeps_ec_words_raw_and_never_claims_rpm_calibration() {
        let mut sample = crate::model::SensorSample::new(
            "acer_ec:fan0.rpm".into(),
            crate::model::SensorRole::FanRpm(0),
            crate::model::SensorKind::Rpm,
            Some(3896.0),
            crate::model::Unit::Raw,
            Some("0x38:0x0f".into()),
            "ec_u16_provisional",
            crate::model::SensorProvider::Ec,
            "acer_ec".into(),
            None,
            None,
            None,
        );
        sample.status = SensorStatus::Ok;
        let snapshot = SensorSnapshot {
            sensors: vec![sample],
            errors: Vec::new(),
            collected_at_ms: now(),
        };
        let telemetry = ValidationTelemetry::from_state(&snapshot, None, now());
        assert_eq!(telemetry.ec_values.len(), 1);
        assert_eq!(telemetry.ec_values[0].unit, "raw");
        assert_eq!(
            telemetry.ec_values[0].raw_value.as_deref(),
            Some("0x38:0x0f")
        );
        let record = ValidationRecord::for_session(
            &session(),
            ValidationEvent::StepVerified,
            now(),
            Some(telemetry),
            Some("validation".into()),
        );
        let text = serde_json::to_string(&record).unwrap();
        assert!(!text.contains("rpm_calibration"));
        assert!(!text.contains("\"value_c\":3896"));
    }

    #[test]
    fn evidence_history_tolerates_only_a_truncated_final_line() {
        let dir = tempfile::tempdir().unwrap();
        let store = EvidenceStore::new(dir.path());
        let session = session();
        let record =
            ValidationRecord::for_session(&session, ValidationEvent::Authorized, now(), None, None);
        store.append(&record).unwrap();
        let path = dir.path().join("session-1.jsonl");
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"incomplete\":").unwrap();
        let history = store.read_with_warnings(Some("session-1")).unwrap();
        assert_eq!(history.records.len(), 1);
        assert!(
            history
                .warnings
                .iter()
                .any(|warning| warning.contains("incomplete final"))
        );

        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"\nnot-json\n").unwrap();
        assert!(store.read_with_warnings(Some("session-1")).is_err());
    }

    #[test]
    fn evidence_history_handles_empty_and_multiple_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = EvidenceStore::new(dir.path());
        let session = session();
        let empty = store.read_with_warnings(Some("session-1")).unwrap();
        assert!(empty.records.is_empty());
        for event in [ValidationEvent::Authorized, ValidationEvent::StepStarted] {
            store
                .append(&ValidationRecord::for_session(
                    &session,
                    event,
                    now(),
                    None,
                    None,
                ))
                .unwrap();
        }
        let history = store.read_with_warnings(Some("session-1")).unwrap();
        assert_eq!(history.records.len(), 2);
        assert!(history.warnings.is_empty());
    }

    #[test]
    fn evidence_store_rejects_path_like_session_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = EvidenceStore::new(dir.path());
        let mut record = ValidationRecord::for_session(
            &session(),
            ValidationEvent::Authorized,
            now(),
            None,
            None,
        );
        record.session_id = "../escape".into();
        assert!(store.append(&record).is_err());
    }
}
