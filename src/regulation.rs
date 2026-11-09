//! Deterministic thermal regulation policy.
//!
//! This module is deliberately backend-independent and pure. It converts a
//! trusted temperature snapshot into target percentages for known fans. It
//! never writes hardware and never treats a target as measured RPM.

use crate::model::{SensorSample, SensorSnapshot};
use crate::policy::FanRole;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    pub temperature_c: f64,
    pub percent: f64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CurveSpec {
    pub points: &'static [CurvePoint],
    pub min_percent: f64,
    pub max_percent: f64,
    pub hysteresis_c: f64,
    pub max_step_percent: f64,
    pub stale_after_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegulationMode {
    Preview,
    Simulated,
}

impl RegulationMode {
    pub fn label(self) -> &'static str {
        match self {
            RegulationMode::Preview => "preview",
            RegulationMode::Simulated => "simulated",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanObservation {
    pub fan_index: u32,
    pub fan_name: String,
    pub role: FanRole,
    pub mapping_ok: bool,
    pub previous_target_percent: Option<f64>,
    pub previous_temperature_c: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetDecision {
    pub fan_index: u32,
    pub fan_name: String,
    pub role: FanRole,
    pub mode: RegulationMode,
    pub temperature_c: Option<f64>,
    pub target_percent: Option<f64>,
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileState {
    pub selected: Option<String>,
    pub mode: Option<RegulationMode>,
    pub evaluated_at_ms: u64,
    pub targets: Vec<TargetDecision>,
    pub reason: Option<String>,
}

pub fn mapping_matches(role: FanRole, sensors: &[String]) -> bool {
    let matches = |words: &[&str]| {
        sensors.iter().any(|sensor| {
            let lower = sensor.to_ascii_lowercase();
            words.iter().any(|word| lower.contains(word))
        })
    };
    match role {
        FanRole::Cpu => matches(&["cpu", "coretemp", "k10temp", "zenpower"]),
        FanRole::Gpu => matches(&["gpu", "amdgpu", "nvidia", "nouveau", "radeon"]),
        FanRole::Unknown => false,
    }
}

pub fn validate_curve(curve: &CurveSpec) -> Result<(), String> {
    if curve.points.len() < 2 {
        return Err("curve must contain at least two points".to_string());
    }
    if !curve.min_percent.is_finite()
        || !curve.max_percent.is_finite()
        || curve.min_percent < 0.0
        || curve.max_percent > 100.0
        || curve.min_percent >= curve.max_percent
    {
        return Err("curve percent bounds are invalid".to_string());
    }
    if !curve.hysteresis_c.is_finite() || curve.hysteresis_c < 0.0 {
        return Err("curve hysteresis is invalid".to_string());
    }
    if !curve.max_step_percent.is_finite() || curve.max_step_percent <= 0.0 {
        return Err("curve rate limit is invalid".to_string());
    }
    if curve.stale_after_ms == 0 {
        return Err("curve staleness window is invalid".to_string());
    }
    let mut previous_temperature = f64::NEG_INFINITY;
    for point in curve.points {
        if !point.temperature_c.is_finite() || !point.percent.is_finite() {
            return Err("curve points must be finite".to_string());
        }
        if point.temperature_c <= previous_temperature {
            return Err("curve temperatures must be strictly increasing".to_string());
        }
        if !(0.0..=100.0).contains(&point.percent) {
            return Err("curve percentages must be between 0 and 100".to_string());
        }
        previous_temperature = point.temperature_c;
    }
    Ok(())
}

pub fn interpolate(curve: &CurveSpec, temperature_c: f64) -> Result<f64, String> {
    validate_curve(curve)?;
    if !temperature_c.is_finite() {
        return Err("temperature is not finite".to_string());
    }
    let first = curve.points.first().expect("validated curve");
    let last = curve.points.last().expect("validated curve");
    if temperature_c <= first.temperature_c {
        return Ok(first.percent.clamp(curve.min_percent, curve.max_percent));
    }
    if temperature_c >= last.temperature_c {
        return Ok(last.percent.clamp(curve.min_percent, curve.max_percent));
    }
    for pair in curve.points.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if temperature_c <= right.temperature_c {
            let span = right.temperature_c - left.temperature_c;
            let ratio = (temperature_c - left.temperature_c) / span;
            let target = left.percent + (right.percent - left.percent) * ratio;
            return Ok(target.clamp(curve.min_percent, curve.max_percent));
        }
    }
    Err("temperature did not match curve segment".to_string())
}

fn sensor_for_role(
    role: FanRole,
    snapshot: &SensorSnapshot,
    now: u64,
    ttl_ms: u64,
) -> Option<&SensorSample> {
    match role {
        FanRole::Cpu => snapshot.cpu_package(now, ttl_ms),
        FanRole::Gpu => snapshot
            .gpu_indexes()
            .into_iter()
            .find_map(|index| snapshot.gpu_die(index, now, ttl_ms)),
        FanRole::Unknown => None,
    }
}

fn unavailable(
    fan: &FanObservation,
    mode: RegulationMode,
    reason: impl Into<String>,
) -> TargetDecision {
    TargetDecision {
        fan_index: fan.fan_index,
        fan_name: fan.fan_name.clone(),
        role: fan.role,
        mode,
        temperature_c: None,
        target_percent: None,
        available: false,
        reason: Some(reason.into()),
    }
}

pub fn evaluate_curve(
    curve: &CurveSpec,
    snapshot: &SensorSnapshot,
    fans: &[FanObservation],
    mode: RegulationMode,
    now: u64,
) -> Result<Vec<TargetDecision>, String> {
    validate_curve(curve)?;
    let mut decisions = Vec::with_capacity(fans.len());
    for fan in fans {
        if fan.role == FanRole::Unknown {
            decisions.push(unavailable(fan, mode, "fan role is ambiguous"));
            continue;
        }
        if !fan.mapping_ok {
            decisions.push(unavailable(
                fan,
                mode,
                "fan sensor mapping does not match the requested role",
            ));
            continue;
        }
        let Some(sensor) = sensor_for_role(fan.role, snapshot, now, curve.stale_after_ms) else {
            decisions.push(unavailable(
                fan,
                mode,
                "no fresh, valid and trusted temperature is available",
            ));
            continue;
        };
        let Some(temperature_c) = sensor.value else {
            decisions.push(unavailable(fan, mode, "temperature value is unavailable"));
            continue;
        };
        let mut target = interpolate(curve, temperature_c)?;
        if let (Some(previous), Some(previous_temp)) =
            (fan.previous_target_percent, fan.previous_temperature_c)
            && (temperature_c - previous_temp).abs() < curve.hysteresis_c
        {
            target = previous;
        }
        if let Some(previous) = fan.previous_target_percent {
            target = target.clamp(
                previous - curve.max_step_percent,
                previous + curve.max_step_percent,
            );
        }
        target = target.clamp(curve.min_percent, curve.max_percent);
        decisions.push(TargetDecision {
            fan_index: fan.fan_index,
            fan_name: fan.fan_name.clone(),
            role: fan.role,
            mode,
            temperature_c: Some(temperature_c),
            target_percent: Some(target),
            available: true,
            reason: Some(format!(
                "{} target calculated; hardware control is not applied",
                mode.label()
            )),
        });
    }
    Ok(decisions)
}

pub fn evaluate_profile(
    curve: &CurveSpec,
    snapshot: &SensorSnapshot,
    fans: &[FanObservation],
    mode: RegulationMode,
    now: u64,
) -> Result<Vec<TargetDecision>, String> {
    evaluate_curve(curve, snapshot, fans, mode, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SensorKind, SensorProvider, SensorRole, SensorStatus, Trust, Unit, now_ms};

    const POINTS: [CurvePoint; 4] = [
        CurvePoint {
            temperature_c: 40.0,
            percent: 20.0,
        },
        CurvePoint {
            temperature_c: 60.0,
            percent: 40.0,
        },
        CurvePoint {
            temperature_c: 80.0,
            percent: 70.0,
        },
        CurvePoint {
            temperature_c: 90.0,
            percent: 100.0,
        },
    ];
    const CURVE: CurveSpec = CurveSpec {
        points: &POINTS,
        min_percent: 20.0,
        max_percent: 100.0,
        hysteresis_c: 3.0,
        max_step_percent: 10.0,
        stale_after_ms: 3_000,
    };

    fn temp(role: SensorRole, value: f64, at: u64, trust: Trust) -> SensorSample {
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
            None,
            None,
            None,
        )
        .with_trust(trust);
        sample.sampled_at_ms = at;
        sample
    }

    fn snapshot(cpu: Option<f64>, gpu: Option<f64>, at: u64) -> SensorSnapshot {
        let mut sensors = Vec::new();
        if let Some(value) = cpu {
            sensors.push(temp(SensorRole::CpuPackage, value, at, Trust::Verified));
        }
        if let Some(value) = gpu {
            sensors.push(temp(SensorRole::GpuDie(0), value, at, Trust::Verified));
        }
        SensorSnapshot {
            sensors,
            errors: Vec::new(),
            collected_at_ms: at,
        }
    }

    fn fan(index: u32, role: FanRole, mapping_ok: bool) -> FanObservation {
        FanObservation {
            fan_index: index,
            fan_name: match role {
                FanRole::Cpu => "CPU fan",
                FanRole::Gpu => "GPU fan",
                FanRole::Unknown => "fan",
            }
            .into(),
            role,
            mapping_ok,
            previous_target_percent: None,
            previous_temperature_c: None,
        }
    }

    #[test]
    fn interpolates_curve_boundaries_and_midpoints() {
        assert_eq!(interpolate(&CURVE, 40.0).unwrap(), 20.0);
        assert_eq!(interpolate(&CURVE, 50.0).unwrap(), 30.0);
        assert_eq!(interpolate(&CURVE, 60.0).unwrap(), 40.0);
        assert_eq!(interpolate(&CURVE, 80.0).unwrap(), 70.0);
        assert_eq!(interpolate(&CURVE, 95.0).unwrap(), 100.0);
    }

    #[test]
    fn clamps_to_floor_and_ceiling() {
        assert_eq!(interpolate(&CURVE, -10.0).unwrap(), 20.0);
        assert_eq!(interpolate(&CURVE, 200.0).unwrap(), 100.0);
    }

    #[test]
    fn hysteresis_holds_target_inside_deadband() {
        let now = now_ms();
        let mut observation = fan(0, FanRole::Cpu, true);
        observation.previous_target_percent = Some(30.0);
        observation.previous_temperature_c = Some(50.0);
        let decisions = evaluate_curve(
            &CURVE,
            &snapshot(Some(52.0), None, now),
            &[observation],
            RegulationMode::Simulated,
            now,
        )
        .unwrap();
        assert_eq!(decisions[0].target_percent, Some(30.0));
    }

    #[test]
    fn rate_limit_and_floor_are_applied() {
        let now = now_ms();
        let mut observation = fan(0, FanRole::Cpu, true);
        observation.previous_target_percent = Some(20.0);
        observation.previous_temperature_c = Some(40.0);
        let decisions = evaluate_curve(
            &CURVE,
            &snapshot(Some(80.0), None, now),
            &[observation],
            RegulationMode::Simulated,
            now,
        )
        .unwrap();
        assert_eq!(decisions[0].target_percent, Some(30.0));
    }

    #[test]
    fn stale_or_missing_temperature_is_unavailable_not_zero() {
        let now = now_ms();
        let stale = snapshot(Some(70.0), None, now.saturating_sub(4_000));
        let decisions = evaluate_curve(
            &CURVE,
            &stale,
            &[fan(0, FanRole::Cpu, true)],
            RegulationMode::Preview,
            now,
        )
        .unwrap();
        assert!(!decisions[0].available);
        assert_eq!(decisions[0].target_percent, None);
        assert!(decisions[0].reason.as_deref().unwrap().contains("fresh"));

        let missing = evaluate_curve(
            &CURVE,
            &snapshot(None, None, now),
            &[fan(0, FanRole::Cpu, true)],
            RegulationMode::Preview,
            now,
        )
        .unwrap();
        assert!(!missing[0].available);
        assert_eq!(missing[0].target_percent, None);
    }

    #[test]
    fn mapping_mismatch_blocks_gpu_target() {
        let now = now_ms();
        let decisions = evaluate_curve(
            &CURVE,
            &snapshot(Some(60.0), Some(60.0), now),
            &[fan(0, FanRole::Cpu, true), fan(1, FanRole::Gpu, false)],
            RegulationMode::Preview,
            now,
        )
        .unwrap();
        assert!(decisions[0].available);
        assert!(!decisions[1].available);
        assert_eq!(decisions[1].target_percent, None);
        assert!(mapping_matches(FanRole::Cpu, &["@CPU".into()]));
        assert!(!mapping_matches(FanRole::Gpu, &["@CPU".into()]));
    }

    #[test]
    fn invalid_curves_are_rejected() {
        let invalid = CurveSpec {
            points: &[
                CurvePoint {
                    temperature_c: 60.0,
                    percent: 30.0,
                },
                CurvePoint {
                    temperature_c: 50.0,
                    percent: 40.0,
                },
            ],
            ..CURVE
        };
        assert!(validate_curve(&invalid).is_err());
        let invalid_floor = CurveSpec {
            min_percent: -1.0,
            ..CURVE
        };
        assert!(validate_curve(&invalid_floor).is_err());
    }

    #[test]
    fn preview_and_simulated_are_distinct_and_never_write() {
        let now = now_ms();
        let input = snapshot(Some(60.0), None, now);
        let observations = [fan(0, FanRole::Cpu, true)];
        let preview =
            evaluate_curve(&CURVE, &input, &observations, RegulationMode::Preview, now).unwrap();
        let simulated = evaluate_curve(
            &CURVE,
            &input,
            &observations,
            RegulationMode::Simulated,
            now,
        )
        .unwrap();
        assert_eq!(preview[0].target_percent, simulated[0].target_percent);
        assert_eq!(preview[0].mode, RegulationMode::Preview);
        assert_eq!(simulated[0].mode, RegulationMode::Simulated);
        assert!(
            preview[0]
                .reason
                .as_deref()
                .unwrap()
                .contains("not applied")
        );
    }

    #[test]
    fn invalid_status_is_not_treated_as_available() {
        let now = now_ms();
        let mut sample = temp(SensorRole::CpuPackage, 60.0, now, Trust::Verified);
        sample.status = SensorStatus::Invalid;
        sample.value = None;
        let snap = SensorSnapshot {
            sensors: vec![sample],
            errors: Vec::new(),
            collected_at_ms: now,
        };
        let decisions = evaluate_curve(
            &CURVE,
            &snap,
            &[fan(0, FanRole::Cpu, true)],
            RegulationMode::Preview,
            now,
        )
        .unwrap();
        assert!(!decisions[0].available);
        assert_eq!(decisions[0].target_percent, None);
    }
}
