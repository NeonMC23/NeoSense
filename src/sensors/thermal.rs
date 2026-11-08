use super::{ProviderOutput, SysfsRoot, parse_f64, plausible_temperature, read_trim};
use crate::model::{
    SensorHealth, SensorKind, SensorProvider, SensorRole, SensorSample, SensorStatus, Trust, Unit,
};
use std::path::{Path, PathBuf};

pub fn collect(root: &SysfsRoot) -> ProviderOutput {
    let base = root.class("thermal");
    let entries = match sorted_dirs(&base) {
        Ok(entries) => entries,
        Err(e) => return ProviderOutput::error(SensorProvider::Thermal, e),
    };

    let mut output = ProviderOutput::default();
    for (position, dir) in entries.iter().enumerate() {
        let zone = read_trim(&dir.join("type")).unwrap_or_else(|_| format!("zone{position}"));
        let raw = read_trim(&dir.join("temp"));
        let role = role_for_zone(&zone, position as u32);
        let trust = if zone.eq_ignore_ascii_case("x86_pkg_temp") {
            Trust::Trusted
        } else {
            Trust::Provisional
        };
        let zone_id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("zone{position}"));
        let id = format!("thermal:{zone}:{zone_id}");
        let path = Some(dir.join("temp").display().to_string());
        let mut sample = SensorSample::new(
            id,
            role,
            SensorKind::Temperature,
            None,
            Unit::Celsius,
            raw.as_ref().ok().cloned(),
            "millidegree_c",
            SensorProvider::Thermal,
            zone.clone(),
            path,
            Some(zone),
            None,
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
                        sample = sample.invalid(format!("température hors plage: {celsius:.1} °C"));
                    }
                }
                Err(e) => sample = sample.invalid(e),
            },
        }
        output.sensors.push(sample);
    }
    output
}

fn sorted_dirs(base: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(base)
        .map_err(|e| format!("{}: {e}", base.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("thermal_zone"))
        })
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn role_for_zone(zone: &str, index: u32) -> SensorRole {
    if zone.eq_ignore_ascii_case("x86_pkg_temp") {
        SensorRole::CpuPackage
    } else {
        SensorRole::ThermalZone(index)
    }
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
    fn x86_package_is_a_cpu_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let z = tmp.path().join("class/thermal/thermal_zone5");
        write(z.join("type"), "x86_pkg_temp\n");
        write(z.join("temp"), "64000\n");
        let out = collect(&SysfsRoot::new(tmp.path()));
        let pkg = out
            .sensors
            .iter()
            .find(|s| s.role == SensorRole::CpuPackage)
            .unwrap();
        assert_eq!(pkg.value, Some(64.0));
        assert_eq!(pkg.health.trust, Trust::Trusted);
    }

    #[test]
    fn acpitz_is_a_thermal_zone_not_cpu_package() {
        let tmp = tempfile::tempdir().unwrap();
        let z = tmp.path().join("class/thermal/thermal_zone0");
        write(z.join("type"), "acpitz\n");
        write(z.join("temp"), "52000\n");
        let out = collect(&SysfsRoot::new(tmp.path()));
        assert_eq!(out.sensors[0].role, SensorRole::ThermalZone(0));
        assert_eq!(out.sensors[0].health.trust, Trust::Provisional);
    }
}
