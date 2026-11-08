pub mod hwmon;
pub mod nvidia;
pub mod power_supply;
pub mod thermal;

use crate::model::{ProviderError, SensorProvider, SensorSample, SensorSnapshot, now_ms};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SysfsRoot {
    pub path: PathBuf,
}

impl Default for SysfsRoot {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/sys"),
        }
    }
}

impl SysfsRoot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn class(&self, name: &str) -> PathBuf {
        self.path.join("class").join(name)
    }
}

#[derive(Debug, Default)]
pub struct ProviderOutput {
    pub sensors: Vec<SensorSample>,
    pub errors: Vec<ProviderError>,
}

impl ProviderOutput {
    pub fn error(provider: SensorProvider, message: impl Into<String>) -> Self {
        Self {
            sensors: Vec::new(),
            errors: vec![ProviderError {
                provider,
                message: message.into(),
            }],
        }
    }
}

/// Collect all supported sensor providers.
///
/// Provider failures are data, not panics. A missing `/sys` tree simply yields
/// no sensors; the caller must treat that as unavailable, never as 0 °C.
pub fn collect_all(root: &SysfsRoot) -> SensorSnapshot {
    let mut sensors = Vec::new();
    let mut errors = Vec::new();

    for output in [
        hwmon::collect(root),
        thermal::collect(root),
        power_supply::collect(root),
        nvidia::collect(),
    ] {
        sensors.extend(output.sensors);
        errors.extend(output.errors);
    }

    SensorSnapshot {
        sensors,
        errors,
        collected_at_ms: now_ms(),
    }
}

pub(crate) fn read_trim(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn parse_f64(raw: &str) -> Result<f64, String> {
    let value: f64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("non-numeric value: {raw:?}"))?;
    if !value.is_finite() {
        return Err(format!("non-finite value: {raw:?}"));
    }
    Ok(value)
}

pub(crate) fn plausible_temperature(value: f64) -> bool {
    (-40.0..=150.0).contains(&value)
}

pub(crate) fn plausible_power(value: f64) -> bool {
    (0.0..=1000.0).contains(&value)
}

pub(crate) fn channel_number(file_name: &str, prefix: &str, suffix: &str) -> Option<u32> {
    let middle = file_name.strip_prefix(prefix)?.strip_suffix(suffix)?;
    middle.parse().ok()
}
