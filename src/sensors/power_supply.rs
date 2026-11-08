use super::{ProviderOutput, SysfsRoot, parse_f64, read_trim};
use crate::model::{
    SensorHealth, SensorKind, SensorProvider, SensorRole, SensorSample, SensorStatus, Trust, Unit,
};
use std::path::{Path, PathBuf};

pub fn collect(root: &SysfsRoot) -> ProviderOutput {
    let base = root.class("power_supply");
    let entries = match sorted_dirs(&base) {
        Ok(entries) => entries,
        Err(e) => return ProviderOutput::error(SensorProvider::PowerSupply, e),
    };

    let mut output = ProviderOutput::default();
    for dir in entries {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "battery".to_string());
        if !name.to_ascii_uppercase().starts_with("BAT") {
            continue;
        }

        let temp_path = dir.join("temp");
        if temp_path.exists() {
            let raw = read_trim(&temp_path);
            let id = format!("power_supply:{name}:temp");
            let mut sample = SensorSample::new(
                id,
                SensorRole::Battery,
                SensorKind::Temperature,
                None,
                Unit::Celsius,
                raw.as_ref().ok().cloned(),
                "tenth_celsius",
                SensorProvider::PowerSupply,
                name.clone(),
                Some(temp_path.display().to_string()),
                None,
                None,
            )
            .with_trust(Trust::Trusted);

            match raw {
                Err(e) => {
                    sample.status = SensorStatus::Unavailable;
                    sample.health = SensorHealth::unavailable(Trust::Trusted);
                    sample.error = Some(e);
                }
                Ok(raw) => match parse_f64(&raw) {
                    Ok(tenths) => {
                        let celsius = tenths / 10.0;
                        if (-20.0..=100.0).contains(&celsius) {
                            sample.value = Some(celsius);
                            sample.status = SensorStatus::Ok;
                            sample.health = SensorHealth::usable(Trust::Trusted);
                        } else {
                            sample = sample.invalid(format!(
                                "battery temperature out of range: {celsius:.1} °C"
                            ));
                        }
                    }
                    Err(e) => sample = sample.invalid(e),
                },
            }
            output.sensors.push(sample);
        }

        let power_path = dir.join("power_now");
        if power_path.exists() {
            let raw = read_trim(&power_path);
            let id = format!("power_supply:{name}:power_now");
            let mut sample = SensorSample::new(
                id,
                SensorRole::Power,
                SensorKind::Power,
                None,
                Unit::Watt,
                raw.as_ref().ok().cloned(),
                "microwatt",
                SensorProvider::PowerSupply,
                name.clone(),
                Some(power_path.display().to_string()),
                None,
                None,
            )
            .with_trust(Trust::Trusted);

            match raw {
                Err(e) => {
                    sample.status = SensorStatus::Unavailable;
                    sample.health = SensorHealth::unavailable(Trust::Trusted);
                    sample.error = Some(e);
                }
                Ok(raw) => match parse_f64(&raw) {
                    Ok(microwatts) => {
                        let watts = microwatts / 1_000_000.0;
                        if (0.0..=1000.0).contains(&watts) {
                            sample.value = Some(watts);
                            sample.status = SensorStatus::Ok;
                            sample.health = SensorHealth::usable(Trust::Trusted);
                        } else {
                            sample = sample.invalid(format!("power out of range: {watts:.1} W"));
                        }
                    }
                    Err(e) => sample = sample.invalid(e),
                },
            }
            output.sensors.push(sample);
        }
    }
    output
}

fn sorted_dirs(base: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(base)
        .map_err(|e| format!("{}: {e}", base.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}
