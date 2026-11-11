//! Read-only inspection of the selected NBFC configuration.
//!
//! NBFC stores one JSON document per model configuration. Reading it answers
//! questions the human `nbfc status` output cannot: which register pair drives
//! each logical fan, which sensor groups feed it, and whether the readback
//! range is actually configured. It never changes NBFC configuration and never
//! writes to the EC.

use crate::nbfc::NbfcSensorConfig;
use crate::policy::{FanRole, fan_role};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcThreshold {
    #[serde(rename = "UpThreshold", default)]
    pub up_threshold: Option<f64>,
    #[serde(rename = "DownThreshold", default)]
    pub down_threshold: Option<f64>,
    #[serde(rename = "FanSpeed", default)]
    pub fan_speed: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcFanConfig {
    #[serde(rename = "ReadRegister", default)]
    pub read_register: Option<u16>,
    #[serde(rename = "WriteRegister", default)]
    pub write_register: Option<u16>,
    #[serde(rename = "MinSpeedValue", default)]
    pub min_speed_value: Option<f64>,
    #[serde(rename = "MaxSpeedValue", default)]
    pub max_speed_value: Option<f64>,
    #[serde(rename = "IndependentReadMinMaxValues", default)]
    pub independent_read_min_max_values: Option<bool>,
    #[serde(rename = "MinSpeedValueRead", default)]
    pub min_speed_value_read: Option<f64>,
    #[serde(rename = "MaxSpeedValueRead", default)]
    pub max_speed_value_read: Option<f64>,
    #[serde(rename = "ResetRequired", default)]
    pub reset_required: Option<bool>,
    #[serde(rename = "FanSpeedResetValue", default)]
    pub fan_speed_reset_value: Option<f64>,
    #[serde(rename = "FanDisplayName", default)]
    pub display_name: String,
    #[serde(rename = "TemperatureThresholds", default)]
    pub temperature_thresholds: Vec<NbfcThreshold>,
}

/// Where NBFC takes the numeric scale used to turn a raw EC read into a
/// percentage.
///
/// NBFC only uses `MinSpeedValueRead`/`MaxSpeedValueRead` when
/// `IndependentReadMinMaxValues` is true. Otherwise it silently falls back to
/// the write range. That distinction matters: a 16-bit tach-like value compared
/// against a 0..100 write range is always out of range and gets clamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadbackRangeSource {
    /// Explicit `MinSpeedValueRead`/`MaxSpeedValueRead` range.
    Independent,
    /// Inferred by NBFC from `MinSpeedValue`/`MaxSpeedValue`.
    WriteRange,
    /// No usable range is declared.
    Invalid,
}

impl NbfcFanConfig {
    /// The effective read range NBFC uses, mirroring `src/fan.c`:
    /// `MinSpeedValueRead`/`MaxSpeedValueRead` when
    /// `IndependentReadMinMaxValues` is true, otherwise `MinSpeedValue`/
    /// `MaxSpeedValue`. Returns absolute endpoints plus the source.
    pub fn effective_read_range(&self) -> (Option<f64>, Option<f64>, ReadbackRangeSource) {
        let independent = self.independent_read_min_max_values.unwrap_or(false);
        let (min, max, source) = if independent {
            (
                self.min_speed_value_read,
                self.max_speed_value_read,
                ReadbackRangeSource::Independent,
            )
        } else {
            (
                self.min_speed_value,
                self.max_speed_value,
                ReadbackRangeSource::WriteRange,
            )
        };
        match (min, max) {
            (Some(a), Some(b)) if a.is_finite() && b.is_finite() && a != b => {
                (Some(a.min(b)), Some(a.max(b)), source)
            }
            _ => (None, None, ReadbackRangeSource::Invalid),
        }
    }

    /// True only when an explicit independent read range is configured. This is
    /// stricter than "NBFC has some range": the inferred write range is usually
    /// the wrong scale for word-sized tach values.
    pub fn readback_configured(&self) -> bool {
        self.effective_read_range().2 == ReadbackRangeSource::Independent
    }

    pub fn register_label(&self) -> String {
        format!(
            "read={} write={}",
            self.read_register
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into()),
            self.write_register
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".into())
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcRegisterWrite {
    #[serde(rename = "WriteMode", default)]
    pub write_mode: String,
    #[serde(rename = "WriteOccasion", default)]
    pub write_occasion: String,
    #[serde(rename = "Register", default)]
    pub register: Option<u16>,
    #[serde(rename = "Value", default)]
    pub value: Option<u16>,
    #[serde(rename = "ResetRequired", default)]
    pub reset_required: Option<bool>,
    #[serde(rename = "ResetValue", default)]
    pub reset_value: Option<u16>,
    #[serde(rename = "ResetWriteMode", default)]
    pub reset_write_mode: String,
    #[serde(rename = "Description", default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcConfigDocument {
    #[serde(rename = "NotebookModel", default)]
    pub notebook_model: String,
    #[serde(rename = "Author", default)]
    pub author: String,
    #[serde(rename = "CriticalTemperature", default)]
    pub critical_temperature: Option<f64>,
    #[serde(rename = "EcPollInterval", default)]
    pub ec_poll_interval_ms: Option<u64>,
    #[serde(rename = "ReadWriteWords", default)]
    pub read_write_words: Option<bool>,
    #[serde(rename = "FanConfigurations", default)]
    pub fans: Vec<NbfcFanConfig>,
    #[serde(rename = "RegisterWriteConfigurations", default)]
    pub register_writes: Vec<NbfcRegisterWrite>,
}

impl NbfcConfigDocument {
    pub fn parse(text: &str) -> Result<Self, String> {
        let document: Self = serde_json::from_str(text)
            .map_err(|e| format!("invalid NBFC configuration JSON: {e}"))?;
        if document.fans.is_empty() {
            return Err("NBFC configuration contains no FanConfigurations".to_string());
        }
        Ok(document)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FanIdentity {
    pub index: u32,
    pub name: String,
    pub role: FanRole,
    pub read_register: Option<u16>,
    pub write_register: Option<u16>,
    pub register_pair: String,
    pub min_speed_value: Option<f64>,
    pub max_speed_value: Option<f64>,
    pub min_speed_value_read: Option<f64>,
    pub max_speed_value_read: Option<f64>,
    pub independent_read_min_max_values: Option<bool>,
    /// True only when an explicit independent read range is configured.
    pub readback_configured: bool,
    /// Absolute endpoints NBFC actually uses for the read range check.
    pub effective_read_min: Option<f64>,
    pub effective_read_max: Option<f64>,
    pub readback_range_source: ReadbackRangeSource,
    pub reset_required: Option<bool>,
    pub fan_speed_reset_value: Option<f64>,
    pub sensor_sources: Vec<String>,
    pub algorithm: Option<String>,
    pub threshold_count: usize,
    /// Identity here is logical: separate registers do not prove separate
    /// physical fans or independent airflow paths.
    pub physical_identity_verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HardwareIdentityReport {
    pub schema: &'static str,
    pub selected_config: String,
    pub config_path: String,
    pub notebook_model: String,
    pub author: String,
    pub critical_temperature: Option<f64>,
    pub ec_poll_interval_ms: Option<u64>,
    pub read_write_words: Option<bool>,
    pub fans: Vec<FanIdentity>,
    pub register_write_count: usize,
    pub warnings: Vec<String>,
    pub hardware_applied: bool,
}

pub fn config_dirs() -> Vec<PathBuf> {
    if let Some(value) = std::env::var_os("NEOSENSE_NBFC_CONFIG_DIRS") {
        let dirs: Vec<PathBuf> = std::env::split_paths(&value).collect();
        if !dirs.is_empty() {
            return dirs;
        }
    }
    vec![
        PathBuf::from("/etc/nbfc/configs"),
        PathBuf::from("/usr/share/nbfc/configs"),
        PathBuf::from("/var/lib/nbfc/configs"),
    ]
}

pub fn find_config(name: &str) -> Result<(PathBuf, NbfcConfigDocument), String> {
    find_config_in_dirs(name, &config_dirs())
}

pub fn find_config_in_dirs(
    name: &str,
    dirs: &[PathBuf],
) -> Result<(PathBuf, NbfcConfigDocument), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("NBFC configuration name is empty".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed == "." || trimmed == ".." {
        return Err(format!("unsafe NBFC configuration name: {name}"));
    }
    let file_name = format!("{trimmed}.json");
    for dir in dirs {
        let path = dir.join(&file_name);
        if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            return NbfcConfigDocument::parse(&text).map(|document| (path, document));
        }
    }
    Err(format!(
        "NBFC configuration '{trimmed}' was not found in {}",
        dirs.iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

pub fn identity_report(
    config_name: &str,
    sensor_config: Option<&NbfcSensorConfig>,
) -> Result<HardwareIdentityReport, String> {
    identity_report_in_dirs(config_name, sensor_config, &config_dirs())
}

pub fn identity_report_in_dirs(
    config_name: &str,
    sensor_config: Option<&NbfcSensorConfig>,
    dirs: &[PathBuf],
) -> Result<HardwareIdentityReport, String> {
    let (path, document) = find_config_in_dirs(config_name, dirs)?;
    let mut warnings = Vec::new();
    let mut fans = Vec::new();
    for (index, fan) in document.fans.iter().enumerate() {
        let index = index as u32;
        let role = fan_role(&fan.display_name);
        let sources = sensor_config
            .and_then(|config| config.fans.iter().find(|item| item.index == index))
            .map(|item| item.sensors.clone())
            .unwrap_or_default();
        let algorithm = sensor_config
            .and_then(|config| config.fans.iter().find(|item| item.index == index))
            .and_then(|item| item.algorithm.clone());
        if role == FanRole::Unknown {
            warnings.push(format!(
                "fan {index} ('{}') has no recognised CPU/GPU role",
                fan.display_name
            ));
        }
        if role == FanRole::Gpu && !sources.iter().any(|source| mentions_gpu(source)) {
            warnings.push(format!(
                "fan {index} is named GPU but its sensor sources ({}) do not include a GPU temperature",
                if sources.is_empty() {
                    "none".to_string()
                } else {
                    sources.join(", ")
                }
            ));
        }
        let (effective_min, effective_max, range_source) = fan.effective_read_range();
        match range_source {
            ReadbackRangeSource::WriteRange => warnings.push(format!(
                "fan {index} ('{}') has IndependentReadMinMaxValues=false; NBFC compares the raw read value against the write range {}..{} and clamps out-of-range values, so CurrentSpeed is not a measurement",
                fan.display_name,
                format_bound(effective_min),
                format_bound(effective_max)
            )),
            ReadbackRangeSource::Invalid => warnings.push(format!(
                "fan {index} ('{}') declares no usable read range; NBFC CurrentSpeed cannot be interpreted as a speed percentage",
                fan.display_name
            )),
            ReadbackRangeSource::Independent => {}
        }
        fans.push(FanIdentity {
            index,
            name: fan.display_name.clone(),
            role,
            read_register: fan.read_register,
            write_register: fan.write_register,
            register_pair: fan.register_label(),
            min_speed_value: fan.min_speed_value,
            max_speed_value: fan.max_speed_value,
            min_speed_value_read: fan.min_speed_value_read,
            max_speed_value_read: fan.max_speed_value_read,
            independent_read_min_max_values: fan.independent_read_min_max_values,
            readback_configured: fan.readback_configured(),
            effective_read_min: effective_min,
            effective_read_max: effective_max,
            readback_range_source: range_source,
            reset_required: fan.reset_required,
            fan_speed_reset_value: fan.fan_speed_reset_value,
            sensor_sources: sources,
            algorithm,
            threshold_count: fan.temperature_thresholds.len(),
            physical_identity_verified: false,
        });
    }
    let pairs: std::collections::HashSet<(Option<u16>, Option<u16>)> = document
        .fans
        .iter()
        .map(|fan| (fan.write_register, fan.read_register))
        .collect();
    if pairs.len() < document.fans.len() {
        warnings.push(
            "multiple logical fans share a read/write register pair; independent control is not proven"
                .to_string(),
        );
    } else if document.fans.len() > 1 {
        warnings.push(
            "separate register pairs exist, but physical fan identity and independent airflow are not verified"
                .to_string(),
        );
    }
    if !warnings
        .iter()
        .any(|warning| warning.contains("tachometer"))
    {
        warnings.push(
            "NBFC's status contract exposes no tachometer/RPM value; EC CurrentSpeed is a percentage derived from readback, not measured RPM"
                .to_string(),
        );
    }
    Ok(HardwareIdentityReport {
        schema: "neosense.hardware_identity/v1",
        selected_config: config_name.to_string(),
        config_path: path.display().to_string(),
        notebook_model: document.notebook_model,
        author: document.author,
        critical_temperature: document.critical_temperature,
        ec_poll_interval_ms: document.ec_poll_interval_ms,
        read_write_words: document.read_write_words,
        fans,
        register_write_count: document.register_writes.len(),
        warnings,
        hardware_applied: false,
    })
}

fn format_bound(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.0}"))
        .unwrap_or_else(|| "?".to_string())
}

fn mentions_gpu(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    ["gpu", "amdgpu", "nvidia", "nouveau", "radeon"]
        .iter()
        .any(|word| lower.contains(word))
}

/// Read-only convenience used by diagnostics. Returns `None` when the selected
/// configuration document cannot be inspected.
pub fn identity_report_if_available(
    selected_config: Option<&str>,
    sensor_config: Option<&NbfcSensorConfig>,
) -> Option<Result<HardwareIdentityReport, String>> {
    selected_config.map(|name| identity_report(name, sensor_config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbfc::{NbfcFanSensor, NbfcSensorConfig};

    const FIXTURE: &str = r#"
    {
      "NotebookModel": "Acer Nitro AN515-57 Custom",
      "Author": "Test",
      "EcPollInterval": 1000,
      "ReadWriteWords": true,
      "CriticalTemperature": 95,
      "FanConfigurations": [
        {
          "ReadRegister": 19, "WriteRegister": 55,
          "MinSpeedValue": 0, "MaxSpeedValue": 100,
          "IndependentReadMinMaxValues": false,
          "MinSpeedValueRead": 0, "MaxSpeedValueRead": 0,
          "ResetRequired": true, "FanSpeedResetValue": 50,
          "FanDisplayName": "CPU fan",
          "TemperatureThresholds": [
            {"UpThreshold": 40, "DownThreshold": 0, "FanSpeed": 0}
          ]
        },
        {
          "ReadRegister": 21, "WriteRegister": 58,
          "MinSpeedValue": 0, "MaxSpeedValue": 100,
          "IndependentReadMinMaxValues": false,
          "MinSpeedValueRead": 0, "MaxSpeedValueRead": 0,
          "ResetRequired": true, "FanSpeedResetValue": 50,
          "FanDisplayName": "GPU fan",
          "TemperatureThresholds": [
            {"UpThreshold": 40, "DownThreshold": 0, "FanSpeed": 0}
          ]
        }
      ],
      "RegisterWriteConfigurations": [
        {"WriteMode": "Set", "WriteOccasion": "OnInitialization", "Register": 3, "Value": 17}
      ]
    }
    "#;

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
                    sensors: vec!["@CPU".into()],
                    algorithm: Some("Average".into()),
                },
            ],
        }
    }

    #[test]
    fn parses_registers_and_roles() {
        let document = NbfcConfigDocument::parse(FIXTURE).unwrap();
        assert_eq!(document.fans.len(), 2);
        assert_eq!(document.fans[0].read_register, Some(19));
        assert_eq!(document.fans[0].write_register, Some(55));
        assert_eq!(document.fans[1].write_register, Some(58));
        assert!(!document.fans[0].readback_configured());
        assert_eq!(
            document.fans[0].effective_read_range(),
            (Some(0.0), Some(100.0), ReadbackRangeSource::WriteRange)
        );
        assert_eq!(document.fans[0].register_label(), "read=19 write=55");
    }

    #[test]
    fn disabled_independent_range_infers_the_write_range() {
        let fan = NbfcConfigDocument::parse(FIXTURE).unwrap().fans[0].clone();
        // The AN515-57 case: read values are present but ignored because the
        // independent flag is false. NBFC then compares raw words to 0..100.
        assert_eq!(fan.min_speed_value_read, Some(0.0));
        assert_eq!(fan.max_speed_value_read, Some(0.0));
        let (min, max, source) = fan.effective_read_range();
        assert_eq!(min, Some(0.0));
        assert_eq!(max, Some(100.0));
        assert_eq!(source, ReadbackRangeSource::WriteRange);
    }

    #[test]
    fn independent_range_overrides_the_write_range() {
        let mut fan = NbfcConfigDocument::parse(FIXTURE).unwrap().fans[0].clone();
        fan.independent_read_min_max_values = Some(true);
        fan.min_speed_value_read = Some(0.0);
        fan.max_speed_value_read = Some(6122.0);
        assert_eq!(
            fan.effective_read_range(),
            (Some(0.0), Some(6122.0), ReadbackRangeSource::Independent)
        );
        assert!(fan.readback_configured());
    }

    #[test]
    fn report_exposes_distinct_registers_shared_sensor_and_invalid_readback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Test.json"), FIXTURE).unwrap();
        let report =
            identity_report_in_dirs("Test", Some(&sensor_config()), &[dir.path().to_path_buf()])
                .unwrap();
        assert_eq!(report.schema, "neosense.hardware_identity/v1");
        assert_eq!(report.fans.len(), 2);
        assert_eq!(report.fans[0].role, FanRole::Cpu);
        assert_eq!(report.fans[1].role, FanRole::Gpu);
        assert_eq!(report.fans[0].write_register, Some(55));
        assert_eq!(report.fans[1].write_register, Some(58));
        assert!(report.fans[0].sensor_sources.contains(&"@CPU".to_string()));
        assert!(!report.fans[0].readback_configured);
        assert_eq!(
            report.fans[0].readback_range_source,
            ReadbackRangeSource::WriteRange
        );
        assert_eq!(report.fans[0].effective_read_max, Some(100.0));
        assert!(!report.fans[0].physical_identity_verified);
        assert!(report.warnings.iter().any(|w| w.contains("CurrentSpeed")));
        assert!(report.warnings.iter().any(|w| w.contains("write range")));
        assert!(report.warnings.iter().any(|w| w.contains("GPU")));
        assert!(report.warnings.iter().any(|w| w.contains("register pairs")));
        assert!(report.warnings.iter().any(|w| w.contains("tachometer")));
        assert!(!report.hardware_applied);
    }

    #[test]
    fn invalid_readback_range_is_detected() {
        let mut fan = NbfcConfigDocument::parse(FIXTURE).unwrap().fans[0].clone();
        fan.independent_read_min_max_values = Some(true);
        fan.min_speed_value_read = Some(20.0);
        fan.max_speed_value_read = Some(80.0);
        assert!(fan.readback_configured());
        fan.max_speed_value_read = Some(20.0);
        assert!(!fan.readback_configured());
        assert_eq!(fan.effective_read_range().2, ReadbackRangeSource::Invalid);
        fan.max_speed_value_read = None;
        assert!(!fan.readback_configured());
        assert_eq!(fan.effective_read_range().2, ReadbackRangeSource::Invalid);
    }

    #[test]
    fn unsafe_config_names_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["", "..", "../etc/passwd", "a/b"] {
            assert!(
                find_config_in_dirs(name, &[dir.path().to_path_buf()]).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn missing_config_is_reported_with_searched_paths() {
        let dir = tempfile::tempdir().unwrap();
        let error = find_config_in_dirs("Missing", &[dir.path().to_path_buf()]).unwrap_err();
        assert!(error.contains("not found"));
        assert!(error.contains("Missing"));
    }

    #[test]
    fn empty_fan_list_is_rejected() {
        let error = NbfcConfigDocument::parse(r#"{"FanConfigurations": []}"#).unwrap_err();
        assert!(error.contains("no FanConfigurations"));
    }

    #[test]
    fn register_write_metadata_is_preserved() {
        let document = NbfcConfigDocument::parse(FIXTURE).unwrap();
        assert_eq!(document.register_writes.len(), 1);
        assert_eq!(document.register_writes[0].description, String::new());
    }
}
