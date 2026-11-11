//! Passive RPM calibration analysis for the P1 validation protocol.
//!
//! This module is pure: it consumes paired external-reference and Acer RPM
//! samples and computes regression statistics. It never claims physical fan
//! identity or actuator validation, and a passing regression is only one part
//! of P1 evidence.

use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "neosense.calibration_analysis/v1";
pub const MIN_SAMPLES: usize = 5;
pub const MAX_SAMPLES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    /// External tachometer in RPM. Absolute-RPM acceptance criteria apply.
    #[default]
    Rpm,
    /// Relative airflow. Only response/trend evidence is available.
    RelativeAirflow,
}

impl ReferenceKind {
    pub fn label(self) -> &'static str {
        match self {
            ReferenceKind::Rpm => "rpm",
            ReferenceKind::RelativeAirflow => "relative_airflow",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationSample {
    /// Independent external measurement (RPM or relative airflow).
    pub reference: f64,
    /// Acer Fan 1 (`fan1_input`) at the same time.
    pub acer_fan1: f64,
    /// Acer Fan 2 (`fan2_input`) at the same time.
    pub acer_fan2: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationInput {
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub instrument: Option<String>,
    #[serde(default)]
    pub instrument_uncertainty: Option<f64>,
    #[serde(default)]
    pub reference_kind: ReferenceKind,
    pub samples: Vec<CalibrationSample>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelFit {
    pub channel: &'static str,
    pub n: usize,
    pub slope: f64,
    pub intercept: f64,
    pub r_squared: f64,
    pub residual_standard_deviation: f64,
    pub slope_95_ci: [f64; 2],
    pub intercept_95_ci: [f64; 2],
    pub accepted: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CalibrationReport {
    pub schema: &'static str,
    pub generated_at_ms: u64,
    pub instrument: Option<String>,
    pub instrument_uncertainty: Option<f64>,
    pub reference_kind: ReferenceKind,
    pub sample_count: usize,
    pub absolute_rpm_calibration: bool,
    pub channels: Vec<ChannelFit>,
    pub notes: Vec<String>,
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

/// Two-sided 95% critical value. A small table avoids a statistics dependency;
/// the normal approximation is used for larger samples.
fn t_critical_95(df: usize) -> f64 {
    const TABLE: [f64; 30] = [
        12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228, 2.201, 2.179, 2.160,
        2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086, 2.080, 2.074, 2.069, 2.064, 2.060, 2.056,
        2.052, 2.048, 2.045, 2.042,
    ];
    if df == 0 {
        return f64::INFINITY;
    }
    TABLE.get(df - 1).copied().unwrap_or(1.96)
}

fn fit(
    channel: &'static str,
    samples: &[CalibrationSample],
    value: impl Fn(&CalibrationSample) -> f64,
    kind: ReferenceKind,
) -> ChannelFit {
    let n = samples.len();
    let xs: Vec<f64> = samples.iter().map(&value).collect();
    let ys: Vec<f64> = samples.iter().map(|sample| sample.reference).collect();
    let mx = mean(&xs);
    let my = mean(&ys);
    let sxx: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let syy: f64 = ys.iter().map(|y| (y - my).powi(2)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { f64::NAN };
    let intercept = my - slope * mx;
    let sse: f64 = xs
        .iter()
        .zip(&ys)
        .map(|(x, y)| (y - (intercept + slope * x)).powi(2))
        .sum();
    let r_squared = if syy > 0.0 {
        1.0 - sse / syy
    } else if sse == 0.0 {
        1.0
    } else {
        0.0
    };
    let df = n.saturating_sub(2);
    let residual_standard_deviation = if df > 0 {
        (sse / df as f64).sqrt()
    } else {
        f64::NAN
    };
    let t = t_critical_95(df);
    let se_slope = if df > 0 && sxx > 0.0 {
        (sse / df as f64 / sxx).sqrt()
    } else {
        f64::NAN
    };
    let se_intercept = if df > 0 && sxx > 0.0 {
        (sse / df as f64 * (1.0 / n as f64 + mx.powi(2) / sxx)).sqrt()
    } else {
        f64::NAN
    };
    let mut reasons = Vec::new();
    if n < MIN_SAMPLES {
        reasons.push(format!(
            "at least {MIN_SAMPLES} paired samples are required; got {n}"
        ));
    }
    if !slope.is_finite() || sxx == 0.0 {
        reasons.push("Acer values do not vary; a regression cannot be fitted".to_string());
    }
    if !r_squared.is_finite() {
        reasons.push("R² is not finite".to_string());
    }
    match kind {
        ReferenceKind::Rpm => {
            if slope.is_finite() && !(0.95..=1.05).contains(&slope) {
                reasons.push(format!("slope {slope:.4} is outside 1.00 ± 0.05"));
            }
            if intercept.is_finite() && intercept.abs() > 50.0 {
                reasons.push(format!("intercept {intercept:.2} RPM is outside ±50 RPM"));
            }
            if r_squared.is_finite() && r_squared < 0.99 {
                reasons.push(format!("R² {r_squared:.4} is below 0.99"));
            }
        }
        ReferenceKind::RelativeAirflow => {
            if r_squared.is_finite() && r_squared < 0.90 {
                reasons.push(format!("relative-airflow R² {r_squared:.4} is below 0.90"));
            }
            if slope.is_finite() && slope <= 0.0 {
                reasons.push("relative airflow does not increase with Acer RPM".to_string());
            }
        }
    }
    ChannelFit {
        channel,
        n,
        slope,
        intercept,
        r_squared,
        residual_standard_deviation,
        slope_95_ci: [slope - t * se_slope, slope + t * se_slope],
        intercept_95_ci: [intercept - t * se_intercept, intercept + t * se_intercept],
        accepted: reasons.is_empty(),
        reasons,
    }
}

pub fn analyze(input: &CalibrationInput) -> Result<CalibrationReport, String> {
    if let Some(schema) = &input.schema
        && schema != "neosense.calibration_input/v1"
    {
        return Err(format!("unsupported calibration schema: {schema}"));
    }
    if input.samples.len() < 2 {
        return Err("at least two paired samples are required".to_string());
    }
    if input.samples.len() > MAX_SAMPLES {
        return Err(format!("at most {MAX_SAMPLES} samples are accepted"));
    }
    if let Some(uncertainty) = input.instrument_uncertainty
        && (!uncertainty.is_finite() || uncertainty < 0.0)
    {
        return Err("instrument uncertainty must be finite and non-negative".to_string());
    }
    for (index, sample) in input.samples.iter().enumerate() {
        for (name, value) in [
            ("reference", sample.reference),
            ("acer_fan1", sample.acer_fan1),
            ("acer_fan2", sample.acer_fan2),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!(
                    "sample {index} {name} must be finite and non-negative"
                ));
            }
        }
    }
    let channels = vec![
        fit(
            "Acer Fan 1",
            &input.samples,
            |s| s.acer_fan1,
            input.reference_kind,
        ),
        fit(
            "Acer Fan 2",
            &input.samples,
            |s| s.acer_fan2,
            input.reference_kind,
        ),
    ];
    let absolute_rpm_calibration = input.reference_kind == ReferenceKind::Rpm
        && channels.iter().all(|channel| channel.accepted);
    let mut notes = vec![
        "Regression analysis only. It does not establish physical fan identity or actuator response."
            .to_string(),
        "Acer RPM remains feedback-only and uncalibrated until this evidence is reviewed.".to_string(),
    ];
    if input.reference_kind == ReferenceKind::RelativeAirflow {
        notes.push(
            "Relative airflow cannot establish absolute RPM calibration; use an external tachometer for that."
                .to_string(),
        );
    }
    Ok(CalibrationReport {
        schema: SCHEMA,
        generated_at_ms: crate::model::now_ms(),
        instrument: input.instrument.clone(),
        instrument_uncertainty: input.instrument_uncertainty,
        reference_kind: input.reference_kind,
        sample_count: input.samples.len(),
        absolute_rpm_calibration,
        channels,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(reference: f64, fan1: f64, fan2: f64) -> CalibrationSample {
        CalibrationSample {
            reference,
            acer_fan1: fan1,
            acer_fan2: fan2,
        }
    }

    fn input(samples: Vec<CalibrationSample>) -> CalibrationInput {
        CalibrationInput {
            schema: Some("neosense.calibration_input/v1".into()),
            instrument: Some("test tachometer".into()),
            instrument_uncertainty: Some(1.0),
            reference_kind: ReferenceKind::Rpm,
            samples,
        }
    }

    #[test]
    fn perfect_linear_data_passes_absolute_criteria() {
        let report = analyze(&input(vec![
            sample(3000.0, 3000.0, 3000.0),
            sample(4000.0, 4000.0, 4000.0),
            sample(5000.0, 5000.0, 5000.0),
            sample(6000.0, 6000.0, 6000.0),
            sample(7000.0, 7000.0, 7000.0),
        ]))
        .unwrap();
        assert!(report.absolute_rpm_calibration);
        assert_eq!(report.sample_count, 5);
        for channel in &report.channels {
            assert!((channel.slope - 1.0).abs() < 1e-9);
            assert!(channel.intercept.abs() < 1e-9);
            assert!((channel.r_squared - 1.0).abs() < 1e-9);
            assert!(channel.accepted);
        }
    }

    #[test]
    fn offset_or_scale_failures_are_explicit() {
        let report = analyze(&input(vec![
            sample(3100.0, 3000.0, 4000.0),
            sample(4100.0, 4000.0, 5000.0),
            sample(5100.0, 5000.0, 6000.0),
            sample(6100.0, 6000.0, 7000.0),
            sample(7100.0, 7000.0, 8000.0),
        ]))
        .unwrap();
        assert!(!report.absolute_rpm_calibration);
        assert!(
            report.channels[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("intercept"))
        );
    }

    #[test]
    fn too_few_samples_are_reported_not_silently_accepted() {
        let report = analyze(&input(vec![
            sample(3000.0, 3000.0, 4000.0),
            sample(4000.0, 4000.0, 5000.0),
        ]))
        .unwrap();
        assert!(!report.channels[0].accepted);
        assert!(
            report.channels[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("at least 5"))
        );
    }

    #[test]
    fn relative_airflow_cannot_claim_absolute_rpm_calibration() {
        let mut input = input(vec![
            sample(1.0, 3000.0, 4000.0),
            sample(2.0, 4000.0, 5000.0),
            sample(3.0, 5000.0, 6000.0),
            sample(4.0, 6000.0, 7000.0),
            sample(5.0, 7000.0, 8000.0),
        ]);
        input.reference_kind = ReferenceKind::RelativeAirflow;
        let report = analyze(&input).unwrap();
        assert!(!report.absolute_rpm_calibration);
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("cannot establish absolute RPM"))
        );
    }

    #[test]
    fn invalid_values_and_constant_acer_data_are_rejected_or_flagged() {
        let mut bad = input(vec![
            sample(1.0, 3000.0, 4000.0),
            sample(2.0, 4000.0, 5000.0),
        ]);
        bad.samples[0].reference = f64::NAN;
        assert!(analyze(&bad).is_err());

        let constant = analyze(&input(vec![
            sample(3000.0, 3000.0, 4000.0),
            sample(4000.0, 3000.0, 4000.0),
            sample(5000.0, 3000.0, 4000.0),
            sample(6000.0, 3000.0, 4000.0),
            sample(7000.0, 3000.0, 4000.0),
        ]))
        .unwrap();
        assert!(!constant.channels[0].accepted);
        assert!(
            constant.channels[0]
                .reasons
                .iter()
                .any(|reason| reason.contains("do not vary"))
        );
    }
}
