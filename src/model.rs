//! Core data model shared by the CLI, daemon and tests.
//!
//! Every hardware reading is represented explicitly: a sensor is not just a
//! number. It carries its provider, role, stable identity, raw value, health and
//! trust. Missing data is `None`, never `0`.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    Untrusted,
    Provisional,
    Trusted,
    Verified,
}

impl Trust {
    pub fn is_at_least(self, other: Trust) -> bool {
        self >= other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorStatus {
    Ok,
    Unavailable,
    Invalid,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "index")]
pub enum SensorRole {
    CpuPackage,
    CpuCore(u32),
    GpuDie(u32),
    GpuHotspot(u32),
    GpuMemory(u32),
    FanRpm(u32),
    Battery,
    ThermalZone(u32),
    Nvme(u32),
    Wifi,
    Power,
    Utilization,
    Unknown,
}

impl std::fmt::Display for SensorRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SensorRole::CpuPackage => write!(f, "cpu.package"),
            SensorRole::CpuCore(i) => write!(f, "cpu.core.{i}"),
            SensorRole::GpuDie(i) => write!(f, "gpu.{i}.die"),
            SensorRole::GpuHotspot(i) => write!(f, "gpu.{i}.hotspot"),
            SensorRole::GpuMemory(i) => write!(f, "gpu.{i}.memory"),
            SensorRole::FanRpm(i) => write!(f, "fan.{i}.rpm"),
            SensorRole::Battery => write!(f, "battery"),
            SensorRole::ThermalZone(i) => write!(f, "thermal.zone.{i}"),
            SensorRole::Nvme(i) => write!(f, "storage.nvme.{i}"),
            SensorRole::Wifi => write!(f, "wifi"),
            SensorRole::Power => write!(f, "power"),
            SensorRole::Utilization => write!(f, "utilization"),
            SensorRole::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    Temperature,
    Rpm,
    Percent,
    Power,
    Utilization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Celsius,
    Rpm,
    Percent,
    Watt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorProvider {
    Hwmon,
    Thermal,
    Nvidia,
    Proc,
    PowerSupply,
}

impl std::fmt::Display for SensorProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SensorProvider::Hwmon => "hwmon",
            SensorProvider::Thermal => "thermal",
            SensorProvider::Nvidia => "nvidia",
            SensorProvider::Proc => "procfs",
            SensorProvider::PowerSupply => "power_supply",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorHealth {
    /// A device/channel was discovered.
    pub detected: bool,
    /// The read/open operation succeeded.
    pub readable: bool,
    /// The value parsed, converted and passed range validation.
    pub valid: bool,
    /// The sample is younger than the configured TTL at report time.
    pub fresh: bool,
    pub trust: Trust,
    /// Valid + fresh + sufficiently trusted. Policy may impose more checks.
    pub usable_for_control: bool,
}

impl SensorHealth {
    pub fn unavailable(trust: Trust) -> Self {
        Self {
            detected: true,
            readable: false,
            valid: false,
            fresh: false,
            trust,
            usable_for_control: false,
        }
    }

    pub fn invalid(trust: Trust) -> Self {
        Self {
            detected: true,
            readable: true,
            valid: false,
            fresh: false,
            trust,
            usable_for_control: false,
        }
    }

    pub fn usable(trust: Trust) -> Self {
        Self {
            detected: true,
            readable: true,
            valid: true,
            fresh: true,
            trust,
            usable_for_control: trust.is_at_least(Trust::Trusted),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorSample {
    pub id: String,
    pub role: SensorRole,
    pub kind: SensorKind,
    pub value: Option<f64>,
    pub unit: Unit,
    pub raw_value: Option<String>,
    pub raw_unit: String,
    pub provider: SensorProvider,
    pub chip: String,
    pub path: Option<String>,
    pub label: Option<String>,
    pub device_id: Option<String>,
    pub sampled_at_ms: u64,
    pub status: SensorStatus,
    pub health: SensorHealth,
    pub error: Option<String>,
}

impl SensorSample {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        role: SensorRole,
        kind: SensorKind,
        value: Option<f64>,
        unit: Unit,
        raw_value: Option<String>,
        raw_unit: &str,
        provider: SensorProvider,
        chip: String,
        path: Option<String>,
        label: Option<String>,
        device_id: Option<String>,
    ) -> Self {
        let valid = value.is_some();
        let trust = match provider {
            SensorProvider::Hwmon | SensorProvider::Nvidia | SensorProvider::Thermal => {
                Trust::Verified
            }
            SensorProvider::PowerSupply => Trust::Trusted,
            SensorProvider::Proc => Trust::Trusted,
        };
        Self {
            id,
            role,
            kind,
            value,
            unit,
            raw_value,
            raw_unit: raw_unit.to_string(),
            provider,
            chip,
            path,
            label,
            device_id,
            sampled_at_ms: now_ms(),
            status: if valid {
                SensorStatus::Ok
            } else {
                SensorStatus::Unavailable
            },
            health: if valid {
                SensorHealth::usable(trust)
            } else {
                SensorHealth::unavailable(trust)
            },
            error: None,
        }
    }

    pub fn invalid(mut self, error: impl Into<String>) -> Self {
        self.value = None;
        self.status = SensorStatus::Invalid;
        self.health = SensorHealth::invalid(self.health.trust);
        self.error = Some(error.into());
        self
    }

    pub fn with_trust(mut self, trust: Trust) -> Self {
        self.health.trust = trust;
        self.health.usable_for_control = self.health.valid && trust.is_at_least(Trust::Trusted);
        self
    }

    pub fn age_ms(&self, now: u64) -> u64 {
        now.saturating_sub(self.sampled_at_ms)
    }

    pub fn is_fresh(&self, now: u64, ttl_ms: u64) -> bool {
        self.age_ms(now) <= ttl_ms
    }

    pub fn is_usable_for_control(&self, now: u64, ttl_ms: u64) -> bool {
        self.health.usable_for_control
            && self.status == SensorStatus::Ok
            && self.value.is_some()
            && self.is_fresh(now, ttl_ms)
    }

    pub fn display_value(&self) -> String {
        match (self.value, self.unit) {
            (Some(v), Unit::Celsius) => format!("{v:.1} °C"),
            (Some(v), Unit::Rpm) => format!("{v:.0} rpm"),
            (Some(v), Unit::Percent) => format!("{v:.1} %"),
            (Some(v), Unit::Watt) => format!("{v:.1} W"),
            (None, _) => "indisponible".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderError {
    pub provider: SensorProvider,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorSnapshot {
    pub sensors: Vec<SensorSample>,
    pub errors: Vec<ProviderError>,
    pub collected_at_ms: u64,
}

impl SensorSnapshot {
    pub fn empty() -> Self {
        Self {
            sensors: Vec::new(),
            errors: Vec::new(),
            collected_at_ms: now_ms(),
        }
    }

    pub fn by_role(&self, role: &SensorRole) -> Option<&SensorSample> {
        self.sensors.iter().find(|s| &s.role == role)
    }

    /// Pick the best CPU package reading: prefer verified/trusted, valid, fresh.
    pub fn cpu_package(&self, now: u64, ttl_ms: u64) -> Option<&SensorSample> {
        self.sensors
            .iter()
            .filter(|s| s.role == SensorRole::CpuPackage)
            .filter(|s| s.is_usable_for_control(now, ttl_ms))
            .max_by_key(|s| s.health.trust)
    }

    /// Pick the best die reading for a GPU index.
    pub fn gpu_die(&self, index: u32, now: u64, ttl_ms: u64) -> Option<&SensorSample> {
        self.sensors
            .iter()
            .filter(|s| s.role == SensorRole::GpuDie(index))
            .filter(|s| s.is_usable_for_control(now, ttl_ms))
            .max_by_key(|s| s.health.trust)
    }

    pub fn gpu_indexes(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .sensors
            .iter()
            .filter_map(|s| match s.role {
                SensorRole::GpuDie(i) | SensorRole::GpuHotspot(i) | SensorRole::GpuMemory(i) => {
                    Some(i)
                }
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn fan_rpm(&self, index: u32) -> Option<&SensorSample> {
        self.by_role(&SensorRole::FanRpm(index))
    }

    pub fn conflicts(&self) -> Vec<String> {
        let mut conflicts = Vec::new();
        // CPU package vs highest core: a large divergence often means a wrong
        // channel or a failed sensor. Do not average them.
        let package = self
            .sensors
            .iter()
            .find(|s| s.role == SensorRole::CpuPackage && s.value.is_some());
        let max_core = self
            .sensors
            .iter()
            .filter(|s| matches!(s.role, SensorRole::CpuCore(_)) && s.value.is_some())
            .filter_map(|s| s.value)
            .fold(None::<f64>, |acc, v| Some(acc.map_or(v, |a| a.max(v))));
        if let (Some(pkg), Some(core)) = (package.and_then(|s| s.value), max_core)
            && (core - pkg).abs() > 20.0
        {
            conflicts.push(format!(
                "CPU package ({pkg:.1} °C) and max core ({core:.1} °C) diverge by more than 20 °C"
            ));
        }
        conflicts
    }
}
