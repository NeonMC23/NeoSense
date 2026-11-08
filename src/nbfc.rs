//! NBFC backend boundary.
//!
//! The backend is deliberately process-based and shell-free. It supports both
//! the human status format of NBFC 0.5.3 and the JSON format introduced in
//! 0.5.4. Parsing is explicit and tested against fixtures; missing fields stay
//! `None` and are never converted to zero.

use crate::util::{CommandOutput, run_command};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NbfcVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl NbfcVersion {
    pub fn parse(input: &str) -> Option<Self> {
        // `nbfc --version` prints "nbfc 0.5.3".
        let token = input.split_whitespace().last()?;
        let mut parts = token.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    pub fn at_least(self, major: u32, minor: u32, patch: u32) -> bool {
        (self.major, self.minor, self.patch) >= (major, minor, patch)
    }
}

impl std::fmt::Display for NbfcVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcFan {
    pub index: u32,
    pub name: String,
    pub temperature: Option<f64>,
    pub critical_temperature: Option<f64>,
    pub auto_mode: Option<bool>,
    pub critical: Option<bool>,
    pub current_speed: Option<f64>,
    pub target_speed: Option<f64>,
    pub requested_speed: Option<f64>,
    pub speed_steps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcStatus {
    pub version: Option<NbfcVersion>,
    pub read_only: Option<bool>,
    pub selected_config: Option<String>,
    pub fans: Vec<NbfcFan>,
    pub warnings: Vec<String>,
}

impl NbfcStatus {
    pub fn fan(&self, index: u32) -> Option<&NbfcFan> {
        self.fans.iter().find(|f| f.index == index)
    }

    pub fn all_auto(&self) -> bool {
        !self.fans.is_empty() && self.fans.iter().all(|f| f.auto_mode == Some(true))
    }

    pub fn any_critical(&self) -> bool {
        self.fans.iter().any(|f| f.critical == Some(true))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcCapabilities {
    pub version: NbfcVersion,
    pub json_status: bool,
    pub reset_ec: bool,
    pub config_apply: bool,
    pub protected_manual: bool,
}

impl NbfcCapabilities {
    pub fn from_version(version: NbfcVersion) -> Self {
        let json_status = version.at_least(0, 5, 4);
        Self {
            version,
            json_status,
            reset_ec: version.at_least(0, 5, 4),
            config_apply: !version.at_least(0, 5, 4),
            protected_manual: version.at_least(0, 5, 4),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NbfcFanSensor {
    pub index: u32,
    pub name: String,
    pub sensors: Vec<String>,
    pub algorithm: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NbfcSensorConfig {
    pub fans: Vec<NbfcFanSensor>,
}

#[derive(Debug, Clone)]
pub struct NbfcCli {
    pub program: String,
    pub timeout: Duration,
}

impl Default for NbfcCli {
    fn default() -> Self {
        Self {
            program: "nbfc".to_string(),
            timeout: Duration::from_secs(5),
        }
    }
}

impl NbfcCli {
    pub fn new(program: impl Into<String>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            timeout,
        }
    }

    fn run(&self, args: &[&str]) -> Result<CommandOutput, String> {
        run_command(&self.program, args, self.timeout)
            .map_err(|e| format!("NBFC indisponible: {e}"))
    }

    pub fn version(&self) -> Result<NbfcVersion, String> {
        let output = self.run(&["--version"])?;
        if !output.success {
            return Err(format!("nbfc --version: {}", output.combined_error()));
        }
        NbfcVersion::parse(&output.stdout)
            .ok_or_else(|| format!("version NBFC illisible: {:?}", output.stdout.trim()))
    }

    pub fn capabilities(&self) -> Result<NbfcCapabilities, String> {
        self.version().map(NbfcCapabilities::from_version)
    }

    pub fn status(&self, version: NbfcVersion) -> Result<NbfcStatus, String> {
        if version.at_least(0, 5, 4) {
            match self.status_json() {
                Ok(status) => return Ok(status),
                Err(json_error) => {
                    // A service can be stopped, or a 0.5.4 JSON schema can be
                    // absent. Try the human format before returning a combined
                    // error, but never hide both failures.
                    return self.status_human().map_err(|human_error| {
                        format!("{json_error}; format texte: {human_error}")
                    });
                }
            }
        }
        self.status_human()
    }

    fn status_json(&self) -> Result<NbfcStatus, String> {
        let output = self.run(&["status", "--json"])?;
        if !output.success {
            return Err(format!("nbfc status --json: {}", output.combined_error()));
        }
        parse_status_json(&output.stdout)
    }

    fn status_human(&self) -> Result<NbfcStatus, String> {
        let output = self.run(&["status"])?;
        if !output.success {
            return Err(format!("nbfc status: {}", output.combined_error()));
        }
        parse_status_human(&output.stdout)
    }

    pub fn set_auto(&self) -> Result<CommandOutput, String> {
        self.run(&["set", "-a"])
    }

    pub fn set_speed(&self, fan: u32, percent: f64) -> Result<CommandOutput, String> {
        let args = set_speed_args(fan, percent);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs)
    }

    pub fn list_configs(&self) -> Result<Vec<String>, String> {
        let output = self.run(&["config", "-l"])?;
        if !output.success {
            return Err(format!("nbfc config -l: {}", output.combined_error()));
        }
        Ok(output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub fn sensors_show(&self) -> Result<NbfcSensorConfig, String> {
        let output = self.run(&["sensors", "show"])?;
        if !output.success {
            return Err(format!("nbfc sensors show: {}", output.combined_error()));
        }
        Ok(parse_sensors_show(&output.stdout))
    }

    pub fn rate_config_json(&self, name: &str) -> Result<String, String> {
        let output = run_command(
            &self.program,
            &["rate-config", "-j", name],
            Duration::from_secs(30),
        )
        .map_err(|e| format!("NBFC indisponible: {e}"))?;
        if !output.success {
            return Err(format!("nbfc rate-config: {}", output.combined_error()));
        }
        Ok(output.stdout)
    }

    /// Return to firmware/EC control when the version exposes `reset-ec`.
    /// This is a privileged, version-specific operation and must only be used
    /// by the fail-safe helper, never by ordinary CLI status commands.
    pub fn reset_ec(&self) -> Result<CommandOutput, String> {
        self.run(&["reset-ec"])
    }
}

/// The exact NBFC argument vector for a fixed-speed request. Kept as a pure
/// function so regression tests can assert `-s` is always present.
pub fn set_speed_args(fan: u32, percent: f64) -> Vec<String> {
    vec![
        "set".to_string(),
        "-f".to_string(),
        fan.to_string(),
        "-s".to_string(),
        format!("{percent:.2}"),
    ]
}

/// Narrow interface used by the supervisor. Tests provide a fake backend and
/// must never touch real hardware.
pub trait FanBackend: Send + Sync {
    fn version(&self) -> Result<NbfcVersion, String>;
    fn status(&self, version: NbfcVersion) -> Result<NbfcStatus, String>;
    fn set_auto(&self) -> Result<CommandOutput, String>;
    fn set_speed(&self, fan: u32, percent: f64) -> Result<CommandOutput, String>;
    fn sensors_show(&self) -> Result<NbfcSensorConfig, String>;
    fn list_configs(&self) -> Result<Vec<String>, String>;
}

impl FanBackend for NbfcCli {
    fn version(&self) -> Result<NbfcVersion, String> {
        NbfcCli::version(self)
    }
    fn status(&self, version: NbfcVersion) -> Result<NbfcStatus, String> {
        NbfcCli::status(self, version)
    }
    fn set_auto(&self) -> Result<CommandOutput, String> {
        NbfcCli::set_auto(self)
    }
    fn set_speed(&self, fan: u32, percent: f64) -> Result<CommandOutput, String> {
        NbfcCli::set_speed(self, fan, percent)
    }
    fn sensors_show(&self) -> Result<NbfcSensorConfig, String> {
        NbfcCli::sensors_show(self)
    }
    fn list_configs(&self) -> Result<Vec<String>, String> {
        NbfcCli::list_configs(self)
    }
}

pub fn parse_status_human(text: &str) -> Result<NbfcStatus, String> {
    let mut status = NbfcStatus {
        version: None,
        read_only: None,
        selected_config: None,
        fans: Vec::new(),
        warnings: Vec::new(),
    };

    for (line_no, line) in text.lines().enumerate() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            continue;
        }

        match key {
            "Read-only" => status.read_only = parse_bool(value),
            "Selected Config Name" | "Selected Config Id" | "SelectedConfigId" => {
                if !value.is_empty() {
                    status.selected_config = Some(value.to_string());
                }
            }
            "Fan Display Name" => {
                if value.is_empty() {
                    status
                        .warnings
                        .push(format!("ligne {}: nom de ventilateur vide", line_no + 1));
                    continue;
                }
                status.fans.push(NbfcFan {
                    index: status.fans.len() as u32,
                    name: value.to_string(),
                    temperature: None,
                    critical_temperature: None,
                    auto_mode: None,
                    critical: None,
                    current_speed: None,
                    target_speed: None,
                    requested_speed: None,
                    speed_steps: None,
                });
            }
            _ => {
                let Some(fan) = status.fans.last_mut() else {
                    continue;
                };
                match key {
                    "Temperature" => fan.temperature = parse_optional_f64(value),
                    "Critical Temperature" => fan.critical_temperature = parse_optional_f64(value),
                    "Auto Control Enabled" => {
                        fan.auto_mode = parse_bool(value);
                        if fan.auto_mode.is_none() {
                            status.warnings.push(format!(
                                "ligne {}: booléen Auto Control Enabled illisible: {value:?}",
                                line_no + 1
                            ));
                        }
                    }
                    "Critical Mode Enabled" => {
                        fan.critical = parse_bool(value);
                    }
                    "Current Fan Speed" => fan.current_speed = parse_optional_f64(value),
                    "Target Fan Speed" => fan.target_speed = parse_optional_f64(value),
                    "Requested Fan Speed" => fan.requested_speed = parse_optional_f64(value),
                    "Fan Speed Steps" => {
                        fan.speed_steps = value.parse::<u32>().ok();
                    }
                    _ => {}
                }
            }
        }
    }

    // A status with no fan blocks is valid when the service has no selected
    // config, but the caller must not invent fan data.
    Ok(status)
}

pub fn parse_status_json(text: &str) -> Result<NbfcStatus, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("JSON NBFC invalide: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "JSON NBFC: objet attendu".to_string())?;

    let read_only = object.get("ReadOnly").and_then(|v| v.as_bool());
    let selected_config = object
        .get("SelectedConfigId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let fans_value = object
        .get("Fans")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "JSON NBFC: tableau Fans manquant".to_string())?;

    let mut fans = Vec::with_capacity(fans_value.len());
    let mut warnings = Vec::new();
    for (index, value) in fans_value.iter().enumerate() {
        let Some(fan) = value.as_object() else {
            warnings.push(format!("Fans[{index}]: objet attendu"));
            continue;
        };
        let name = fan
            .get("Name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            warnings.push(format!("Fans[{index}]: nom manquant"));
        }
        fans.push(NbfcFan {
            index: index as u32,
            name,
            temperature: fan.get("Temperature").and_then(|v| v.as_f64()),
            critical_temperature: fan.get("CriticalTemperature").and_then(|v| v.as_f64()),
            auto_mode: fan.get("AutoMode").and_then(|v| v.as_bool()),
            critical: fan.get("Critical").and_then(|v| v.as_bool()),
            current_speed: fan.get("CurrentSpeed").and_then(|v| v.as_f64()),
            target_speed: fan.get("TargetSpeed").and_then(|v| v.as_f64()),
            requested_speed: fan.get("RequestedSpeed").and_then(|v| v.as_f64()),
            speed_steps: fan
                .get("SpeedSteps")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
        });
    }

    Ok(NbfcStatus {
        version: None,
        read_only,
        selected_config,
        fans,
        warnings,
    })
}

pub fn parse_sensors_show(text: &str) -> NbfcSensorConfig {
    let mut config = NbfcSensorConfig::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let Some(open) = trimmed.find('(') else {
                continue;
            };
            let Some(relative_close) = trimmed[open..].find(')') else {
                continue;
            };
            let close = open + relative_close;
            let index = trimmed[1..open]
                .trim()
                .parse::<u32>()
                .unwrap_or(config.fans.len() as u32);
            let name = trimmed[open + 1..close].trim().to_string();
            config.fans.push(NbfcFanSensor {
                index,
                name,
                sensors: Vec::new(),
                algorithm: None,
            });
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Sensors:") {
            if let Some(fan) = config.fans.last_mut() {
                fan.sensors = value
                    .replace("(default)", "")
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        } else if let Some(value) = trimmed.strip_prefix("Algorithm:") {
            if let Some(fan) = config.fans.last_mut() {
                fan.algorithm = Some(value.trim().to_string());
            }
        }
    }
    config
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_optional_f64(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUMAN: &str = "Read-only                : false\n\
Selected Config Name     : Acer Nitro AN515-57 Custom\n\
\n\
Fan Display Name         : CPU fan\n\
Temperature              : 49.88\n\
Auto Control Enabled     : true\n\
Critical Mode Enabled    : false\n\
Current Fan Speed        : 100.00\n\
Target Fan Speed         : 35.00\n\
Fan Speed Steps          : 100\n\
\n\
Fan Display Name         : GPU fan\n\
Temperature              : 49.88\n\
Auto Control Enabled     : false\n\
Critical Mode Enabled    : false\n\
Current Fan Speed        : 100.00\n\
Target Fan Speed         : 40.00\n\
Fan Speed Steps          : 100\n";

    #[test]
    fn parses_real_human_output_and_lowercase_false() {
        let status = parse_status_human(HUMAN).unwrap();
        assert_eq!(status.fans.len(), 2);
        assert_eq!(status.fans[0].name, "CPU fan");
        assert_eq!(status.fans[0].auto_mode, Some(true));
        assert_eq!(status.fans[1].auto_mode, Some(false));
        assert_eq!(status.fans[1].target_speed, Some(40.0));
        assert!(!status.all_auto());
    }

    #[test]
    fn parses_json_format() {
        let json = r#"{
          "PID": 42,
          "SelectedConfigId": "Acer Nitro AN515-57 Custom",
          "ReadOnly": false,
          "Fans": [
            {"Name":"CPU fan","Temperature":58.0,"CriticalTemperature":95.0,
             "AutoMode":true,"Critical":false,"CurrentSpeed":35.0,
             "TargetSpeed":35.0,"RequestedSpeed":-1.0,"SpeedSteps":100}
          ]
        }"#;
        let status = parse_status_json(json).unwrap();
        assert_eq!(
            status.selected_config.as_deref(),
            Some("Acer Nitro AN515-57 Custom")
        );
        assert_eq!(status.fans.len(), 1);
        assert_eq!(status.fans[0].auto_mode, Some(true));
        assert_eq!(status.fans[0].requested_speed, Some(-1.0));
    }

    #[test]
    fn missing_fields_are_none_not_zero() {
        let status = parse_status_human("Fan Display Name : CPU fan\n").unwrap();
        assert_eq!(status.fans.len(), 1);
        assert_eq!(status.fans[0].temperature, None);
        assert_eq!(status.fans[0].target_speed, None);
        assert_eq!(status.fans[0].auto_mode, None);
    }

    #[test]
    fn empty_status_does_not_fabricate_fans() {
        let status = parse_status_human("Read-only : false\n").unwrap();
        assert!(status.fans.is_empty());
        assert!(!status.all_auto());
    }

    #[test]
    fn sensors_show_maps_source_per_fan() {
        let cfg = parse_sensors_show(
            "#0 (CPU fan):\n  Sensors:   @CPU (default)\n  Algorithm: Average (default)\n\
             #1 (GPU fan):\n  Sensors:   @CPU (default)\n  Algorithm: Average (default)\n",
        );
        assert_eq!(cfg.fans.len(), 2);
        assert_eq!(cfg.fans[1].sensors, vec!["@CPU"]);
    }

    #[test]
    fn set_speed_always_contains_speed_option() {
        let args = set_speed_args(0, 50.0);
        assert_eq!(args, vec!["set", "-f", "0", "-s", "50.00"]);
        assert!(args.iter().any(|a| a == "-s"));
    }

    #[test]
    fn version_parses() {
        assert_eq!(
            NbfcVersion::parse("nbfc 0.5.3").unwrap(),
            NbfcVersion {
                major: 0,
                minor: 5,
                patch: 3
            }
        );
        assert!(
            NbfcCapabilities::from_version(NbfcVersion::parse("nbfc 0.5.4").unwrap())
                .protected_manual
        );
        assert!(
            !NbfcCapabilities::from_version(NbfcVersion::parse("nbfc 0.5.3").unwrap())
                .protected_manual
        );
    }
}
