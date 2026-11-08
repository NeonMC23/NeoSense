use super::ProviderOutput;
use crate::model::{
    ProviderError, SensorHealth, SensorKind, SensorProvider, SensorRole, SensorSample,
    SensorStatus, Trust, Unit, now_ms,
};
use crate::util::run_command;
use std::time::Duration;

const QUERY_ARGS: &[&str] = &[
    "--query-gpu=index,name,temperature.gpu,temperature.memory,utilization.gpu,power.draw",
    "--format=csv,noheader,nounits",
];

pub fn collect() -> ProviderOutput {
    if !std::path::Path::new("/dev/nvidia0").exists() && !command_in_path("nvidia-smi") {
        // A machine without NVIDIA hardware should not produce a warning for a
        // missing optional provider.
        return ProviderOutput::default();
    }
    match run_command("nvidia-smi", QUERY_ARGS, Duration::from_secs(2)) {
        Ok(output) if output.success => {
            let (sensors, errors) = parse_csv(&output.stdout);
            ProviderOutput {
                sensors,
                errors: errors
                    .into_iter()
                    .map(|message| ProviderError {
                        provider: SensorProvider::Nvidia,
                        message,
                    })
                    .collect(),
            }
        }
        Ok(output) => ProviderOutput::error(
            SensorProvider::Nvidia,
            format!("nvidia-smi: {}", output.combined_error()),
        ),
        Err(e) => ProviderOutput::error(SensorProvider::Nvidia, e),
    }
}

/// Parse the stable CSV query used above. This is public for hermetic tests;
/// tests must never invoke nvidia-smi on the host.
pub fn parse_csv(text: &str) -> (Vec<SensorSample>, Vec<String>) {
    let mut sensors = Vec::new();
    let mut errors = Vec::new();

    for (line_no, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if fields.len() < 6 {
            errors.push(format!(
                "ligne nvidia-smi {} invalide: champs insuffisants",
                line_no + 1
            ));
            continue;
        }
        let index = fields[0].parse::<u32>().unwrap_or(line_no as u32);
        let name = fields[1].to_string();
        let device = format!("nvidia:{index}:{name}");
        let sampled_at = now_ms();

        push_optional_temperature(
            &mut sensors,
            &mut errors,
            index,
            &name,
            &device,
            "temperature.gpu",
            fields[2],
            SensorRole::GpuDie(index),
            Trust::Verified,
            sampled_at,
        );
        push_optional_temperature(
            &mut sensors,
            &mut errors,
            index,
            &name,
            &device,
            "temperature.memory",
            fields[3],
            SensorRole::GpuMemory(index),
            Trust::Trusted,
            sampled_at,
        );
        push_percent(&mut sensors, &mut errors, index, &name, &device, fields[4]);
        push_power(&mut sensors, &mut errors, index, &name, &device, fields[5]);
    }

    (sensors, errors)
}

#[allow(clippy::too_many_arguments)]
fn push_optional_temperature(
    sensors: &mut Vec<SensorSample>,
    errors: &mut Vec<String>,
    index: u32,
    name: &str,
    device: &str,
    field: &str,
    raw: &str,
    role: SensorRole,
    trust: Trust,
    sampled_at: u64,
) {
    let id = format!("nvidia:{index}:{field}");
    let mut sample = SensorSample::new(
        id,
        role,
        SensorKind::Temperature,
        None,
        Unit::Celsius,
        Some(raw.to_string()),
        "celsius",
        SensorProvider::Nvidia,
        name.to_string(),
        Some(format!("nvidia-smi:index={index}")),
        Some(field.to_string()),
        Some(device.to_string()),
    )
    .with_trust(trust);
    sample.sampled_at_ms = sampled_at;

    match parse_optional(raw) {
        Ok(Some(celsius)) if (-40.0..=150.0).contains(&celsius) => {
            sample.value = Some(celsius);
            sample.status = SensorStatus::Ok;
            sample.health = SensorHealth::usable(trust);
        }
        Ok(Some(celsius)) => {
            sample = sample.invalid(format!("temperature out of range: {celsius:.1} °C"));
        }
        Ok(None) => {
            sample.status = SensorStatus::Unavailable;
            sample.health = SensorHealth::unavailable(trust);
            sample.error = Some("not exposed by nvidia-smi".to_string());
        }
        Err(e) => {
            errors.push(format!("GPU {index} {field}: {e}"));
            sample = sample.invalid(e);
        }
    }
    sensors.push(sample);
}

fn push_percent(
    sensors: &mut Vec<SensorSample>,
    errors: &mut Vec<String>,
    index: u32,
    name: &str,
    device: &str,
    raw: &str,
) {
    let id = format!("nvidia:{index}:utilization.gpu");
    let mut sample = SensorSample::new(
        id,
        SensorRole::Utilization,
        SensorKind::Utilization,
        None,
        Unit::Percent,
        Some(raw.to_string()),
        "percent",
        SensorProvider::Nvidia,
        name.to_string(),
        Some(format!("nvidia-smi:index={index}")),
        Some("utilization.gpu".to_string()),
        Some(device.to_string()),
    )
    .with_trust(Trust::Trusted);

    match parse_optional(raw) {
        Ok(Some(value)) if (0.0..=100.0).contains(&value) => {
            sample.value = Some(value);
            sample.status = SensorStatus::Ok;
            sample.health = SensorHealth::usable(Trust::Trusted);
        }
        Ok(Some(value)) => sample = sample.invalid(format!("utilisation hors plage: {value}")),
        Ok(None) => {
            sample.status = SensorStatus::Unavailable;
            sample.health = SensorHealth::unavailable(Trust::Trusted);
            sample.error = Some("not exposed by nvidia-smi".to_string());
        }
        Err(e) => {
            errors.push(format!("GPU {index} utilization: {e}"));
            sample = sample.invalid(e);
        }
    }
    sensors.push(sample);
}

fn push_power(
    sensors: &mut Vec<SensorSample>,
    errors: &mut Vec<String>,
    index: u32,
    name: &str,
    device: &str,
    raw: &str,
) {
    let id = format!("nvidia:{index}:power.draw");
    let mut sample = SensorSample::new(
        id,
        SensorRole::Power,
        SensorKind::Power,
        None,
        Unit::Watt,
        Some(raw.to_string()),
        "watt",
        SensorProvider::Nvidia,
        name.to_string(),
        Some(format!("nvidia-smi:index={index}")),
        Some("power.draw".to_string()),
        Some(device.to_string()),
    )
    .with_trust(Trust::Trusted);

    match parse_optional(raw) {
        Ok(Some(value)) if (0.0..=1000.0).contains(&value) => {
            sample.value = Some(value);
            sample.status = SensorStatus::Ok;
            sample.health = SensorHealth::usable(Trust::Trusted);
        }
        Ok(Some(value)) => sample = sample.invalid(format!("puissance hors plage: {value}")),
        Ok(None) => {
            sample.status = SensorStatus::Unavailable;
            sample.health = SensorHealth::unavailable(Trust::Trusted);
            sample.error = Some("not exposed by nvidia-smi".to_string());
        }
        Err(e) => {
            errors.push(format!("GPU {index} power: {e}"));
            sample = sample.invalid(e);
        }
    }
    sensors.push(sample);
}

fn command_in_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

fn parse_optional(raw: &str) -> Result<Option<f64>, String> {
    let raw = raw.trim();
    if raw.eq_ignore_ascii_case("n/a")
        || raw.eq_ignore_ascii_case("na")
        || raw.eq_ignore_ascii_case("[n/a]")
        || raw.is_empty()
    {
        return Ok(None);
    }
    let value: f64 = raw
        .parse()
        .map_err(|_| format!("non-numeric value: {raw:?}"))?;
    if !value.is_finite() {
        return Err(format!("non-finite value: {raw:?}"));
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_gpus_and_preserves_identity() {
        let csv = "0, NVIDIA GeForce RTX 3050 Mobile, 54, 61, 3, 8.5\n\
                   1, NVIDIA GeForce RTX 4090, 70, 80, 99, 350.0\n";
        let (sensors, errors) = parse_csv(csv);
        assert!(errors.is_empty());
        let gpu0 = sensors
            .iter()
            .find(|s| s.role == SensorRole::GpuDie(0))
            .unwrap();
        let gpu1 = sensors
            .iter()
            .find(|s| s.role == SensorRole::GpuDie(1))
            .unwrap();
        assert_eq!(gpu0.value, Some(54.0));
        assert_eq!(gpu1.value, Some(70.0));
        assert!(sensors.iter().any(|s| s.role == SensorRole::GpuMemory(0)));
        assert!(sensors.iter().any(|s| s.role == SensorRole::Utilization));
    }

    #[test]
    fn n_a_is_unavailable_not_zero() {
        let (sensors, _) = parse_csv("0, GPU, N/A, N/A, N/A, N/A\n");
        let die = sensors
            .iter()
            .find(|s| s.role == SensorRole::GpuDie(0))
            .unwrap();
        assert_eq!(die.value, None);
        assert_eq!(die.status, SensorStatus::Unavailable);
        assert!(!die.health.usable_for_control);
    }

    #[test]
    fn malformed_line_is_reported_not_panicked() {
        let (sensors, errors) = parse_csv("not,a,valid,line\n");
        assert!(sensors.is_empty());
        assert_eq!(errors.len(), 1);
    }
}
