//! User-facing thermal profile catalogue.
//!
//! Dynamic thermal profiles are temperature-driven curves. They are distinct
//! from the historical fixed-speed presets in [`crate::presets`]. This release
//! keeps every closed-loop curve in preview/simulation mode: no curve is a
//! validated hardware default, and selecting one never writes fan state.

use crate::control::{ControlStage, FanReadiness};
use crate::custom_profiles::CustomProfile;
use crate::regulation::{CurvePoint, CurveSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAvailability {
    Available,
    Preview,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAction {
    SafeAuto,
    ClosedLoop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    Builtin,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalProfile {
    pub name: String,
    pub title: String,
    pub description: String,
    pub availability: ProfileAvailability,
    pub action: ProfileAction,
    pub origin: ProfileOrigin,
    /// Fan roles selected by a custom profile. Built-in profiles evaluate
    /// every backend fan with a valid role.
    #[serde(default)]
    pub fan_roles: Option<Vec<crate::policy::FanRole>>,
    pub reason: Option<String>,
    pub safety: String,
    /// Reference or user curve used for preview/simulation only. Never a
    /// hardware default until this machine and backend are independently
    /// validated.
    pub curve: Option<CurveSpec>,
}

fn quiet_curve() -> CurveSpec {
    CurveSpec::from_slice(
        &[
            CurvePoint {
                temperature_c: 40.0,
                percent: 20.0,
            },
            CurvePoint {
                temperature_c: 55.0,
                percent: 30.0,
            },
            CurvePoint {
                temperature_c: 70.0,
                percent: 50.0,
            },
            CurvePoint {
                temperature_c: 82.0,
                percent: 75.0,
            },
            CurvePoint {
                temperature_c: 90.0,
                percent: 100.0,
            },
        ],
        20.0,
        100.0,
        3.0,
        10.0,
        3_000,
    )
}

fn balanced_curve() -> CurveSpec {
    CurveSpec::from_slice(
        &[
            CurvePoint {
                temperature_c: 40.0,
                percent: 25.0,
            },
            CurvePoint {
                temperature_c: 55.0,
                percent: 40.0,
            },
            CurvePoint {
                temperature_c: 70.0,
                percent: 65.0,
            },
            CurvePoint {
                temperature_c: 82.0,
                percent: 90.0,
            },
            CurvePoint {
                temperature_c: 90.0,
                percent: 100.0,
            },
        ],
        20.0,
        100.0,
        3.0,
        10.0,
        3_000,
    )
}

fn performance_curve() -> CurveSpec {
    CurveSpec::from_slice(
        &[
            CurvePoint {
                temperature_c: 40.0,
                percent: 35.0,
            },
            CurvePoint {
                temperature_c: 55.0,
                percent: 55.0,
            },
            CurvePoint {
                temperature_c: 70.0,
                percent: 80.0,
            },
            CurvePoint {
                temperature_c: 82.0,
                percent: 100.0,
            },
            CurvePoint {
                temperature_c: 90.0,
                percent: 100.0,
            },
        ],
        20.0,
        100.0,
        3.0,
        10.0,
        3_000,
    )
}

pub fn builtins() -> Vec<ThermalProfile> {
    vec![
        ThermalProfile {
            name: "safe-auto".to_string(),
            title: "Automatic (NBFC)".to_string(),
            description:
                "Hands control to the NBFC automatic curve and verifies the automatic state."
                    .to_string(),
            availability: ProfileAvailability::Available,
            action: ProfileAction::SafeAuto,
            origin: ProfileOrigin::Builtin,
            fan_roles: None,
            reason: None,
            safety: "No manual speed command; physical fan speed is not guaranteed.".to_string(),
            curve: None,
        },
        ThermalProfile {
            name: "quiet".to_string(),
            title: "Quiet".to_string(),
            description: "Closed-loop low-noise policy with thermal floors and ceilings."
                .to_string(),
            availability: ProfileAvailability::Preview,
            action: ProfileAction::ClosedLoop,
            origin: ProfileOrigin::Builtin,
            fan_roles: None,
            reason: Some(
                "Preview only: curve, hysteresis and fan mapping are not validated on this hardware."
                    .to_string(),
            ),
            safety: "Not applicable until sensors, fans and readback are validated.".to_string(),
            curve: Some(quiet_curve()),
        },
        ThermalProfile {
            name: "balanced".to_string(),
            title: "Balanced".to_string(),
            description: "Closed-loop balance between acoustics and heat dissipation.".to_string(),
            availability: ProfileAvailability::Preview,
            action: ProfileAction::ClosedLoop,
            origin: ProfileOrigin::Builtin,
            fan_roles: None,
            reason: Some(
                "Preview only: curve not validated and manual control unavailable on NBFC 0.5.3."
                    .to_string(),
            ),
            safety: "Not applicable; no fixed-speed writes.".to_string(),
            curve: Some(balanced_curve()),
        },
        ThermalProfile {
            name: "performance".to_string(),
            title: "Performance".to_string(),
            description: "Closed-loop policy prioritising heat dissipation.".to_string(),
            availability: ProfileAvailability::Preview,
            action: ProfileAction::ClosedLoop,
            origin: ProfileOrigin::Builtin,
            fan_roles: None,
            reason: Some(
                "Preview only: thermal validation and physical readback are required.".to_string(),
            ),
            safety: "Not applicable; does not replace NBFC/firmware protections.".to_string(),
            curve: Some(performance_curve()),
        },
    ]
}

/// Convert a validated user profile into the same evaluation model as a
/// built-in curve. The resulting profile is always preview/simulation-only.
pub fn custom_to_thermal(profile: &CustomProfile) -> ThermalProfile {
    ThermalProfile {
        name: profile.name.clone(),
        title: profile.title.clone(),
        description: profile.description.clone(),
        availability: ProfileAvailability::Preview,
        action: ProfileAction::ClosedLoop,
        origin: ProfileOrigin::Custom,
        fan_roles: Some(profile.fan_roles.clone()),
        reason: Some(
            "User-defined curve: preview/simulation only until it is validated against this backend."
                .to_string(),
        ),
        safety: "Passes the same supervisor policy, capability and mapping gates as built-in curves."
            .to_string(),
        curve: Some(profile.curve_spec()),
    }
}

pub fn find(name: &str) -> Option<ThermalProfile> {
    builtins().into_iter().find(|profile| profile.name == name)
}

pub fn resolve(name: &str, customs: &[CustomProfile]) -> Option<ThermalProfile> {
    find(name).or_else(|| {
        customs
            .iter()
            .find(|profile| profile.name == name)
            .map(custom_to_thermal)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Compatible,
    CompatibleWithWarnings,
    PreviewOnly,
    Unsupported,
    BlockedForSafety,
}

impl Compatibility {
    pub fn label(self) -> &'static str {
        match self {
            Compatibility::Compatible => "compatible",
            Compatibility::CompatibleWithWarnings => "compatible with warnings",
            Compatibility::PreviewOnly => "preview only",
            Compatibility::Unsupported => "unsupported",
            Compatibility::BlockedForSafety => "blocked for safety",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Compatibility::Compatible => "compatible",
            Compatibility::CompatibleWithWarnings => "compat. warn",
            Compatibility::PreviewOnly => "preview",
            Compatibility::Unsupported => "unsupported",
            Compatibility::BlockedForSafety => "blocked",
        }
    }

    pub fn can_apply(self) -> bool {
        matches!(
            self,
            Compatibility::Compatible | Compatibility::CompatibleWithWarnings
        )
    }
}

#[derive(Debug, Clone)]
pub struct ProfileContext {
    pub nbfc_available: bool,
    pub service_active: bool,
    pub cpu_trusted: bool,
    pub gpu_present: bool,
    pub gpu_fan_mapping_ok: bool,
    pub rpm_available: bool,
    pub ec_readback_reliable: bool,
    pub protected_manual: bool,
    pub access_controlled: bool,
    pub read_only: bool,
    /// Current control-readiness stage, shared with diagnostics and the TUI.
    pub control_stage: ControlStage,
    /// Per-fan policy eligibility and explicit blocker reasons.
    pub fan_readiness: Vec<FanReadiness>,
}

impl ProfileContext {
    pub fn from_view(view: &crate::runtime::RuntimeView) -> Self {
        let now = crate::model::now_ms();
        let ttl = 5_000;
        let cpu_trusted = view.snapshot.cpu_package(now, ttl).is_some();
        let gpu_present = !view.snapshot.gpu_indexes().is_empty();
        let rpm_available = view
            .snapshot
            .sensors
            .iter()
            .any(|s| s.is_usable_for_feedback(now, ttl));
        let (gpu_fan_mapping_ok, ec_readback_reliable) = match &view.status {
            Some(status) => {
                let mut gpu_fan_mapping_ok = true;
                let mut ec_readback_reliable = !status.fans.is_empty();
                for fan in &status.fans {
                    let role = crate::policy::fan_role(&fan.name);
                    if role == crate::policy::FanRole::Gpu {
                        let source = view
                            .sensor_config
                            .as_ref()
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
                        gpu_fan_mapping_ok &= has_gpu;
                    }
                    if let (Some(current), Some(target)) = (fan.current_speed, fan.target_speed) {
                        ec_readback_reliable &=
                            (0.0..=100.0).contains(&current) && (current - target).abs() <= 20.0;
                    } else {
                        ec_readback_reliable = false;
                    }
                }
                (gpu_fan_mapping_ok, ec_readback_reliable)
            }
            None => (false, false),
        };
        let control = crate::control::assess_view(view);
        Self {
            nbfc_available: view.status.is_some() && view.version.is_some(),
            service_active: view.service.is_active(),
            cpu_trusted,
            gpu_present,
            gpu_fan_mapping_ok,
            rpm_available,
            ec_readback_reliable,
            protected_manual: view
                .capabilities
                .as_ref()
                .map(|caps| caps.protected_manual)
                .unwrap_or(false),
            access_controlled: view
                .capabilities
                .as_ref()
                .map(|caps| caps.access_controlled)
                .unwrap_or(false),
            read_only: view
                .status
                .as_ref()
                .and_then(|status| status.read_only)
                .unwrap_or(false),
            control_stage: control.stage,
            fan_readiness: control.fans,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileEvaluation {
    pub profile: ThermalProfile,
    pub compatibility: Compatibility,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
    /// Fans this profile would address that are currently policy-eligible.
    pub eligible_fans: Vec<u32>,
    /// Fans this profile would address that are currently blocked.
    pub blocked_fans: Vec<u32>,
}

pub fn evaluate_all(context: &ProfileContext) -> Vec<ProfileEvaluation> {
    builtins()
        .into_iter()
        .map(|profile| evaluate(profile, context))
        .collect()
}

/// Evaluate built-ins followed by persistent user profiles. Invalid custom
/// profiles are rejected by the store before they reach this function.
pub fn evaluate_all_with(
    context: &ProfileContext,
    customs: &[CustomProfile],
) -> Vec<ProfileEvaluation> {
    let mut profiles = builtins();
    profiles.extend(customs.iter().map(custom_to_thermal));
    profiles
        .into_iter()
        .map(|profile| evaluate(profile, context))
        .collect()
}

pub fn evaluate(profile: ThermalProfile, context: &ProfileContext) -> ProfileEvaluation {
    let mut reasons = Vec::new();
    let mut warnings = Vec::new();
    let selected_roles = profile.fan_roles.as_ref();
    let relevant_fans: Vec<&FanReadiness> = context
        .fan_readiness
        .iter()
        .filter(|fan| selected_roles.is_none_or(|roles| roles.contains(&fan.role)))
        .collect();
    let eligible_fans = relevant_fans
        .iter()
        .filter(|fan| fan.policy_eligible)
        .map(|fan| fan.index)
        .collect::<Vec<_>>();
    let blocked_fans = relevant_fans
        .iter()
        .filter(|fan| !fan.policy_eligible)
        .map(|fan| fan.index)
        .collect::<Vec<_>>();
    let compatibility = match profile.action {
        ProfileAction::SafeAuto => {
            if !context.nbfc_available {
                reasons.push("NBFC status is unavailable.".to_string());
            }
            if !context.service_active {
                reasons.push("NBFC service is not active.".to_string());
            }
            if context.read_only {
                reasons.push(
                    "NBFC is in read-only mode and cannot apply automatic control.".to_string(),
                );
            }
            if !context.cpu_trusted {
                reasons.push("No trustworthy CPU temperature is available.".to_string());
            }
            if context.gpu_present && !context.gpu_fan_mapping_ok {
                warnings.push(
                    "GPU fan is not driven by a GPU sensor; GPU-specific control remains blocked."
                        .to_string(),
                );
            }
            if !context.gpu_present {
                warnings.push("No GPU temperature sensor is available.".to_string());
            }
            if !context.rpm_available {
                warnings.push(
                    "No fan tachometer is available; physical verification is limited.".to_string(),
                );
            }
            if !context.ec_readback_reliable {
                warnings.push(
                    "EC speed readback is missing or implausible; it is not an RPM.".to_string(),
                );
            }
            if !context.access_controlled {
                warnings.push(
                    "The NBFC socket is not access-controlled; another local client can bypass the NeoSense supervisor."
                        .to_string(),
                );
            }
            if reasons.is_empty() {
                if warnings.is_empty() {
                    Compatibility::Compatible
                } else {
                    Compatibility::CompatibleWithWarnings
                }
            } else {
                Compatibility::BlockedForSafety
            }
        }
        ProfileAction::ClosedLoop => {
            reasons.push(
                "Closed-loop curve is not implemented or validated on this hardware/backend."
                    .to_string(),
            );
            if !context.protected_manual {
                reasons.push(
                    "Protected manual control requires NBFC >= 0.5.4 and verified sensors."
                        .to_string(),
                );
            }
            if !context.access_controlled {
                reasons.push(
                    "The NBFC socket is not access-controlled; the supervisor is not an exclusive writer."
                        .to_string(),
                );
            }
            if !context.gpu_fan_mapping_ok {
                reasons.push("GPU fan sensor mapping is not verified.".to_string());
            }
            if !context.rpm_available {
                reasons
                    .push("No RPM/tachometer is available for physical verification.".to_string());
            }
            for fan in &relevant_fans {
                if !fan.policy_eligible {
                    reasons.push(format!(
                        "Fan #{} {} is blocked: {}",
                        fan.index,
                        fan.name,
                        fan.reasons.join("; ")
                    ));
                }
            }
            Compatibility::PreviewOnly
        }
    };
    ProfileEvaluation {
        profile,
        compatibility,
        reasons,
        warnings,
        eligible_fans,
        blocked_fans,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fan(index: u32, name: &str, role: crate::policy::FanRole) -> FanReadiness {
        FanReadiness {
            index,
            name: name.to_string(),
            role,
            policy_eligible: true,
            mapping_ok: true,
            sources: vec![if role == crate::policy::FanRole::Gpu {
                "@GPU".to_string()
            } else {
                "@CPU".to_string()
            }],
            temperature_status: crate::control::RequirementStatus::Satisfied,
            reasons: Vec::new(),
            validation_unverified: vec!["physical fan identity is unverified".to_string()],
        }
    }

    fn block_gpu(ctx: &mut ProfileContext) {
        ctx.gpu_fan_mapping_ok = false;
        let gpu = &mut ctx.fan_readiness[1];
        gpu.policy_eligible = false;
        gpu.mapping_ok = false;
        gpu.sources = vec!["@CPU".to_string()];
        gpu.reasons
            .push("GPU fan sources [\"@CPU\"] do not match role GPU".to_string());
    }

    fn context() -> ProfileContext {
        ProfileContext {
            nbfc_available: true,
            service_active: true,
            cpu_trusted: true,
            gpu_present: true,
            gpu_fan_mapping_ok: true,
            rpm_available: true,
            ec_readback_reliable: true,
            protected_manual: true,
            access_controlled: true,
            read_only: false,
            control_stage: ControlStage::HardwareDetected,
            fan_readiness: vec![
                fan(0, "CPU fan", crate::policy::FanRole::Cpu),
                fan(1, "GPU fan", crate::policy::FanRole::Gpu),
            ],
        }
    }

    fn sample_custom(name: &str) -> CustomProfile {
        CustomProfile {
            name: name.to_string(),
            title: "Test curve".to_string(),
            description: "test".to_string(),
            points: vec![
                CurvePoint {
                    temperature_c: 40.0,
                    percent: 25.0,
                },
                CurvePoint {
                    temperature_c: 80.0,
                    percent: 80.0,
                },
            ],
            min_percent: 20.0,
            max_percent: 100.0,
            hysteresis_c: 2.0,
            max_step_percent: 10.0,
            stale_after_ms: 3_000,
            fan_roles: vec![crate::policy::FanRole::Cpu],
        }
    }

    #[test]
    fn compatibility_short_labels_fit_compact_tables() {
        assert_eq!(
            Compatibility::CompatibleWithWarnings.short_label(),
            "compat. warn"
        );
        assert_eq!(Compatibility::BlockedForSafety.short_label(), "blocked");
    }

    #[test]
    fn only_safe_auto_is_applicable() {
        let profiles = builtins();
        let applicable: Vec<&str> = profiles
            .iter()
            .filter(|p| p.availability == ProfileAvailability::Available)
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(applicable, vec!["safe-auto"]);
    }

    #[test]
    fn previews_have_a_reason() {
        for profile in builtins().into_iter().filter(|p| p.name != "safe-auto") {
            assert!(profile.reason.is_some(), "{} lacks a reason", profile.name);
            assert_ne!(profile.action, ProfileAction::SafeAuto);
            assert_eq!(profile.origin, ProfileOrigin::Builtin);
        }
    }

    #[test]
    fn reference_curves_are_valid_and_simulation_only() {
        for profile in builtins().into_iter().filter(|p| p.name != "safe-auto") {
            if let Some(curve) = profile.curve {
                crate::regulation::validate_curve(&curve).unwrap();
                assert!(profile.availability == ProfileAvailability::Preview);
            }
        }
    }

    #[test]
    fn custom_profiles_join_the_catalogue_as_preview_only() {
        let customs = vec![sample_custom("mine")];
        let evaluations = evaluate_all_with(&context(), &customs);
        assert_eq!(evaluations.len(), builtins().len() + 1);
        let custom = evaluations
            .iter()
            .find(|evaluation| evaluation.profile.name == "mine")
            .unwrap();
        assert_eq!(custom.profile.origin, ProfileOrigin::Custom);
        assert_eq!(custom.compatibility, Compatibility::PreviewOnly);
        assert!(!custom.compatibility.can_apply());
        assert!(custom.profile.curve.is_some());
    }

    #[test]
    fn safe_auto_is_compatible_with_full_evidence() {
        let evaluation = evaluate(find("safe-auto").unwrap(), &context());
        assert_eq!(evaluation.compatibility, Compatibility::Compatible);
        assert!(evaluation.compatibility.can_apply());
    }

    #[test]
    fn missing_cpu_blocks_safe_auto() {
        let mut ctx = context();
        ctx.cpu_trusted = false;
        let evaluation = evaluate(find("safe-auto").unwrap(), &ctx);
        assert_eq!(evaluation.compatibility, Compatibility::BlockedForSafety);
        assert!(!evaluation.compatibility.can_apply());
    }

    #[test]
    fn closed_loop_profiles_are_preview_only() {
        for name in ["quiet", "balanced", "performance"] {
            let evaluation = evaluate(find(name).unwrap(), &context());
            assert_eq!(evaluation.compatibility, Compatibility::PreviewOnly);
            assert!(!evaluation.compatibility.can_apply());
        }
    }

    #[test]
    fn read_only_nbfc_blocks_safe_auto() {
        let mut ctx = context();
        ctx.read_only = true;
        let evaluation = evaluate(find("safe-auto").unwrap(), &ctx);
        assert_eq!(evaluation.compatibility, Compatibility::BlockedForSafety);
    }

    #[test]
    fn rpm_feedback_never_upgrades_a_closed_loop_profile_to_hardware() {
        let mut ctx = context();
        ctx.rpm_available = true;
        block_gpu(&mut ctx);
        let evaluation = evaluate(find("quiet").unwrap(), &ctx);
        assert_eq!(evaluation.compatibility, Compatibility::PreviewOnly);
        assert!(!evaluation.compatibility.can_apply());
        assert!(
            evaluation
                .reasons
                .iter()
                .any(|reason| reason.contains("GPU fan sensor mapping"))
        );
        assert_eq!(evaluation.eligible_fans, vec![0]);
        assert_eq!(evaluation.blocked_fans, vec![1]);
        assert!(
            evaluation
                .reasons
                .iter()
                .any(|reason| reason.contains("Fan #1") && reason.contains("@CPU"))
        );
    }

    #[test]
    fn custom_profile_fan_roles_filter_the_readiness_report() {
        let mut ctx = context();
        block_gpu(&mut ctx);
        let custom = sample_custom("cpu-only");
        let evaluation = evaluate(custom_to_thermal(&custom), &ctx);
        assert_eq!(evaluation.eligible_fans, vec![0]);
        assert!(evaluation.blocked_fans.is_empty());
        assert!(
            !evaluation
                .reasons
                .iter()
                .any(|reason| reason.contains("Fan #1"))
        );
    }

    #[test]
    fn missing_gpu_mapping_is_a_warning_not_success_without_evidence() {
        let mut ctx = context();
        block_gpu(&mut ctx);
        let evaluation = evaluate(find("safe-auto").unwrap(), &ctx);
        assert_eq!(
            evaluation.compatibility,
            Compatibility::CompatibleWithWarnings
        );
        assert!(evaluation.compatibility.can_apply());
        assert!(evaluation.warnings.iter().any(|w| w.contains("GPU fan")));
    }
}
