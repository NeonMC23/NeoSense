use super::{
    ProviderOutput, SysfsRoot, channel_number, parse_f64, plausible_power, plausible_temperature,
    read_trim,
};
use crate::model::{
    SensorHealth, SensorKind, SensorProvider, SensorRole, SensorSample, SensorStatus, Trust,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const GPU_CHIPS: &[&str] = &["amdgpu", "nouveau", "radeon"];
const CPU_CHIPS: &[&str] = &["coretemp", "k10temp", "zenpower"];

pub fn collect(root: &SysfsRoot) -> ProviderOutput {
    let base = root.class("hwmon");
    let entries = match sorted_dirs(&base) {
        Ok(entries) => entries,
        Err(e) => return ProviderOutput::error(SensorProvider::Hwmon, e),
    };

    // Build a stable GPU index map from device identity, never from hwmon
    // enumeration order. hwmon numbers are not stable across boots.
    let mut gpu_devices: Vec<String> = entries
        .iter()
        .filter_map(|dir| {
            let name = read_trim(&dir.join("name")).ok()?;
            if GPU_CHIPS.contains(&name.as_str()) {
                Some(parse_device_id(dir))
            } else {
                None
            }
        })
        .collect();
    gpu_devices.sort();
    gpu_devices.dedup();
    let gpu_indexes: HashMap<String, u32> = gpu_devices
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), i as u32))
        .collect();

    let mut output = ProviderOutput::default();

    for dir in entries {
        let chip = match read_trim(&dir.join("name")) {
            Ok(name) => name,
            Err(e) => {
                output.errors.push(crate::model::ProviderError {
                    provider: SensorProvider::Hwmon,
                    message: e,
                });
                continue;
            }
        };
        let device_id = parse_device_id(&dir);
        let gpu_index = gpu_indexes.get(&device_id).copied().unwrap_or(0);
        let files = match sorted_files(&dir) {
            Ok(files) => files,
            Err(e) => {
                output.errors.push(crate::model::ProviderError {
                    provider: SensorProvider::Hwmon,
                    message: e,
                });
                continue;
            }
        };

        for file in files {
            let Some(name) = file
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };

            if let Some(channel) = channel_number(&name, "temp", "_input") {
                let label = read_trim(&dir.join(format!("temp{channel}_label"))).ok();
                let raw_path = dir.join(&name);
                let raw = read_trim(&raw_path);
                let (role, trust) =
                    role_for_temperature(&chip, label.as_deref(), channel, gpu_index);
                let unit = "millidegree_c";
                let sample = temperature_sample(
                    &chip, &device_id, &raw_path, channel, label, role, trust, raw, unit,
                );
                output.sensors.push(sample);
            } else if let Some(channel) = channel_number(&name, "fan", "_input") {
                let label = read_trim(&dir.join(format!("fan{channel}_label"))).ok();
                let raw_path = dir.join(&name);
                let sample = fan_sample(&chip, &device_id, &raw_path, channel, label);
                output.sensors.push(sample);
            } else if let Some(channel) = channel_number(&name, "power", "_input") {
                let raw_path = dir.join(&name);
                let sample = power_sample(&chip, &device_id, &raw_path, channel);
                output.sensors.push(sample);
            }
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

fn sorted_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    Ok(files)
}

fn parse_device_id(dir: &Path) -> String {
    dir.join("device")
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| "unknown".to_string())
        })
}

#[allow(clippy::too_many_arguments)]
fn temperature_sample(
    chip: &str,
    device_id: &str,
    raw_path: &Path,
    channel: u32,
    label: Option<String>,
    role: SensorRole,
    trust: Trust,
    raw: Result<String, String>,
    raw_unit: &str,
) -> SensorSample {
    let id = format!("hwmon:{chip}:{device_id}:temp{channel}");
    let path = Some(raw_path.display().to_string());
    let mut sample = SensorSample::new(
        id,
        role,
        SensorKind::Temperature,
        None,
        crate::model::Unit::Celsius,
        raw.as_ref().ok().cloned(),
        raw_unit,
        SensorProvider::Hwmon,
        chip.to_string(),
        path,
        label,
        Some(device_id.to_string()),
    )
    .with_trust(trust);

    match raw {
        Err(e) => {
            sample.status = SensorStatus::Unavailable;
            sample.health = SensorHealth::unavailable(trust);
            sample.error = Some(e);
        }
        Ok(raw) => match parse_f64(&raw) {
            Ok(millidegrees) => {
                let celsius = millidegrees / 1000.0;
                if plausible_temperature(celsius) {
                    sample.value = Some(celsius);
                    sample.status = SensorStatus::Ok;
                    sample.health = SensorHealth::usable(trust);
                } else {
                    sample = sample.invalid(format!("temperature out of range: {celsius:.1} °C"));
                }
            }
            Err(e) => sample = sample.invalid(e),
        },
    }
    sample
}

fn fan_sample(
    chip: &str,
    device_id: &str,
    raw_path: &Path,
    channel: u32,
    label: Option<String>,
) -> SensorSample {
    let id = format!("hwmon:{chip}:{device_id}:fan{channel}");
    let path = Some(raw_path.display().to_string());
    let raw = read_trim(raw_path);
    let mut sample = SensorSample::new(
        id,
        SensorRole::FanRpm(channel.saturating_sub(1)),
        SensorKind::Rpm,
        None,
        crate::model::Unit::Rpm,
        raw.as_ref().ok().cloned(),
        "rpm",
        SensorProvider::Hwmon,
        chip.to_string(),
        path,
        label,
        Some(device_id.to_string()),
    )
    .with_trust(Trust::Trusted);

    match raw {
        Err(e) => {
            sample.status = SensorStatus::Unavailable;
            sample.health = SensorHealth::unavailable(Trust::Trusted);
            sample.error = Some(e);
        }
        Ok(raw) => match parse_f64(&raw) {
            Ok(rpm) if (0.0..=30_000.0).contains(&rpm) => {
                sample.value = Some(rpm);
                sample.status = SensorStatus::Ok;
                sample.health = SensorHealth::usable(Trust::Trusted);
            }
            Ok(rpm) => sample = sample.invalid(format!("vitesse de ventilateur hors plage: {rpm}")),
            Err(e) => sample = sample.invalid(e),
        },
    }
    sample
}

fn power_sample(chip: &str, device_id: &str, raw_path: &Path, channel: u32) -> SensorSample {
    let id = format!("hwmon:{chip}:{device_id}:power{channel}");
    let path = Some(raw_path.display().to_string());
    let raw = read_trim(raw_path);
    let mut sample = SensorSample::new(
        id,
        SensorRole::Power,
        SensorKind::Power,
        None,
        crate::model::Unit::Watt,
        raw.as_ref().ok().cloned(),
        "microwatt",
        SensorProvider::Hwmon,
        chip.to_string(),
        path,
        None,
        Some(device_id.to_string()),
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
                if plausible_power(watts) {
                    sample.value = Some(watts);
                    sample.status = SensorStatus::Ok;
                    sample.health = SensorHealth::usable(Trust::Trusted);
                } else {
                    sample = sample.invalid(format!("puissance hors plage: {watts:.1} W"));
                }
            }
            Err(e) => sample = sample.invalid(e),
        },
    }
    sample
}

fn role_for_temperature(
    chip: &str,
    label: Option<&str>,
    channel: u32,
    gpu_index: u32,
) -> (SensorRole, Trust) {
    let label = label.unwrap_or("");
    match chip {
        "coretemp" => {
            if label.starts_with("Package id") || label == "Package" {
                (SensorRole::CpuPackage, Trust::Verified)
            } else if let Some(index) = number_after(label, "Core ") {
                (SensorRole::CpuCore(index), Trust::Verified)
            } else if label.is_empty() && channel == 1 {
                // Some kernels/drivers omit labels; channel 1 is the
                // conventional package channel. Mark it trusted, not verified.
                (SensorRole::CpuPackage, Trust::Trusted)
            } else {
                (SensorRole::Unknown, Trust::Provisional)
            }
        }
        "k10temp" | "zenpower" => {
            if label.eq_ignore_ascii_case("Tdie") || label.eq_ignore_ascii_case("Package") {
                (SensorRole::CpuPackage, Trust::Verified)
            } else if label.eq_ignore_ascii_case("Tctl") {
                // Tctl may include a control offset. It is usable only as a
                // fallback; Tdie is preferred when present.
                (SensorRole::CpuPackage, Trust::Trusted)
            } else if label.to_ascii_lowercase().contains("tccd")
                || label.to_ascii_lowercase().contains("ccd")
            {
                (
                    SensorRole::CpuCore(channel.saturating_sub(1)),
                    Trust::Verified,
                )
            } else if label.is_empty() && channel == 1 {
                (SensorRole::CpuPackage, Trust::Trusted)
            } else {
                (SensorRole::Unknown, Trust::Provisional)
            }
        }
        "amdgpu" | "nouveau" | "radeon" => match label.to_ascii_lowercase().as_str() {
            "edge" => (SensorRole::GpuDie(gpu_index), Trust::Verified),
            "junction" | "hotspot" => (SensorRole::GpuHotspot(gpu_index), Trust::Verified),
            "mem" | "memory" => (SensorRole::GpuMemory(gpu_index), Trust::Verified),
            _ if label.is_empty() && channel == 1 => {
                (SensorRole::GpuDie(gpu_index), Trust::Provisional)
            }
            _ => (SensorRole::Unknown, Trust::Provisional),
        },
        "nvme" => (SensorRole::Nvme(0), Trust::Trusted),
        "iwlwifi_1" | "iwlwifi" => (SensorRole::Wifi, Trust::Provisional),
        "acpitz" => (
            SensorRole::ThermalZone(channel.saturating_sub(1)),
            Trust::Provisional,
        ),
        chip if CPU_CHIPS.contains(&chip) => (SensorRole::Unknown, Trust::Provisional),
        _ => (SensorRole::Unknown, Trust::Untrusted),
    }
}

fn number_after(value: &str, prefix: &str) -> Option<u32> {
    value.strip_prefix(prefix)?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: impl AsRef<Path>, value: &str) {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, value).unwrap();
    }

    #[test]
    fn coretemp_package_and_cores_are_identified() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path().join("class/hwmon/hwmon0");
        write(h.join("name"), "coretemp\n");
        write(h.join("temp1_input"), "58000\n");
        write(h.join("temp1_label"), "Package id 0\n");
        write(h.join("temp2_input"), "54000\n");
        write(h.join("temp2_label"), "Core 0\n");

        let root = SysfsRoot::new(tmp.path());
        let out = collect(&root);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let pkg = out
            .sensors
            .iter()
            .find(|s| s.role == SensorRole::CpuPackage)
            .unwrap();
        assert_eq!(pkg.value, Some(58.0));
        assert_eq!(pkg.health.trust, Trust::Verified);
        let core = out
            .sensors
            .iter()
            .find(|s| s.role == SensorRole::CpuCore(0))
            .unwrap();
        assert_eq!(core.value, Some(54.0));
    }

    #[test]
    fn wifi_is_never_assigned_the_gpu_role() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path().join("class/hwmon/hwmon7");
        write(h.join("name"), "iwlwifi_1\n");
        write(h.join("temp1_input"), "54000\n");

        let out = collect(&SysfsRoot::new(tmp.path()));
        assert_eq!(out.sensors.len(), 1);
        assert_eq!(out.sensors[0].role, SensorRole::Wifi);
        assert!(
            !out.sensors
                .iter()
                .any(|s| matches!(s.role, SensorRole::GpuDie(_)))
        );
    }

    #[test]
    fn invalid_temperature_is_not_a_normal_value() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path().join("class/hwmon/hwmon0");
        write(h.join("name"), "coretemp\n");
        write(h.join("temp1_input"), "999000\n");
        write(h.join("temp1_label"), "Package id 0\n");

        let out = collect(&SysfsRoot::new(tmp.path()));
        let pkg = out
            .sensors
            .iter()
            .find(|s| s.role == SensorRole::CpuPackage)
            .unwrap();
        assert_eq!(pkg.value, None);
        assert_eq!(pkg.status, SensorStatus::Invalid);
        assert!(!pkg.health.usable_for_control);
    }

    #[test]
    fn missing_input_is_not_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let h = tmp.path().join("class/hwmon/hwmon0");
        write(h.join("name"), "coretemp\n");
        write(h.join("temp1_label"), "Package id 0\n");
        // No temp1_input file at all: nothing is discovered for this channel.
        let out = collect(&SysfsRoot::new(tmp.path()));
        assert!(out.sensors.is_empty());
    }
}
