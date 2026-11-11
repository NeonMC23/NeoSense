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

    pub fn short_label(self) -> &'static str {
        match self {
            Trust::Untrusted => "untrusted",
            Trust::Provisional => "prov.",
            Trust::Trusted => "trusted",
            Trust::Verified => "verified",
        }
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
    /// Valid + fresh + trusted temperature source eligible to influence
    /// temperature regulation. RPM and other non-temperature sources are
    /// never eligible for regulation.
    #[serde(default)]
    pub usable_for_regulation: bool,
    /// Valid + fresh + trusted physical measurement eligible to be displayed
    /// and compared as observed fan feedback. This does not enable control.
    #[serde(default)]
    pub usable_for_feedback: bool,
    /// The value comes from a hardware measurement, not a requested,
    /// calculated, simulated or backend-echoed value.
    #[serde(default)]
    pub physical_measurement: bool,
    /// Deprecated compatibility aggregate. Equal to `usable_for_regulation`.
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
            usable_for_regulation: false,
            usable_for_feedback: false,
            physical_measurement: false,
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
            usable_for_regulation: false,
            usable_for_feedback: false,
            physical_measurement: false,
            usable_for_control: false,
        }
    }

    pub fn usable(trust: Trust, kind: SensorKind, physical_measurement: bool) -> Self {
        let eligible = trust.is_at_least(Trust::Trusted);
        let usable_for_regulation = eligible && kind == SensorKind::Temperature;
        let usable_for_feedback = eligible && physical_measurement && kind == SensorKind::Rpm;
        Self {
            detected: true,
            readable: true,
            valid: true,
            fresh: true,
            trust,
            usable_for_regulation,
            usable_for_feedback,
            physical_measurement,
            usable_for_control: usable_for_regulation,
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
        // Every current provider reports a real hardware/sysfs measurement.
        // A future derived or simulated source must set this false.
        let physical_measurement = matches!(
            provider,
            SensorProvider::Hwmon
                | SensorProvider::Thermal
                | SensorProvider::Nvidia
                | SensorProvider::PowerSupply
                | SensorProvider::Proc
        );
        let mut health = if valid {
            SensorHealth::usable(trust, kind, physical_measurement)
        } else {
            SensorHealth::unavailable(trust)
        };
        health.physical_measurement = physical_measurement;
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
            health,
            error: None,
        }
    }

    pub fn invalid(mut self, error: impl Into<String>) -> Self {
        self.value = None;
        self.status = SensorStatus::Invalid;
        let physical_measurement = self.health.physical_measurement;
        self.health = SensorHealth::invalid(self.health.trust);
        self.health.physical_measurement = physical_measurement;
        self.error = Some(error.into());
        self
    }

    pub fn with_trust(mut self, trust: Trust) -> Self {
        self.health.trust = trust;
        let eligible = self.health.valid && trust.is_at_least(Trust::Trusted);
        self.health.usable_for_regulation = eligible && self.kind == SensorKind::Temperature;
        self.health.usable_for_feedback =
            eligible && self.health.physical_measurement && self.kind == SensorKind::Rpm;
        self.health.usable_for_control = self.health.usable_for_regulation;
        self
    }

    /// Mark a successfully read physical sample as usable.
    pub fn mark_usable(&mut self, trust: Trust) {
        self.health = SensorHealth::usable(trust, self.kind, self.health.physical_measurement);
    }

    /// Mark a previously detected physical sample as unreadable while keeping
    /// its provenance.
    pub fn mark_unavailable(&mut self, trust: Trust) {
        let physical_measurement = self.health.physical_measurement;
        self.health = SensorHealth::unavailable(trust);
        self.health.physical_measurement = physical_measurement;
    }

    pub fn age_ms(&self, now: u64) -> u64 {
        now.saturating_sub(self.sampled_at_ms)
    }

    pub fn is_fresh(&self, now: u64, ttl_ms: u64) -> bool {
        self.age_ms(now) <= ttl_ms
    }

    /// Valid, fresh and trusted temperature source eligible for regulation.
    pub fn is_usable_for_regulation(&self, now: u64, ttl_ms: u64) -> bool {
        self.health.usable_for_regulation
            && self.status == SensorStatus::Ok
            && self.value.is_some()
            && self.is_fresh(now, ttl_ms)
    }

    /// Valid, fresh and trusted physical RPM measurement eligible as feedback.
    /// This never enables control by itself.
    pub fn is_usable_for_feedback(&self, now: u64, ttl_ms: u64) -> bool {
        self.health.usable_for_feedback
            && self.status == SensorStatus::Ok
            && self.value.is_some()
            && self.is_fresh(now, ttl_ms)
    }

    /// Deprecated compatibility alias for regulation eligibility.
    pub fn is_usable_for_control(&self, now: u64, ttl_ms: u64) -> bool {
        self.is_usable_for_regulation(now, ttl_ms)
    }

    /// Neutral display name. Acer hwmon fan channels have no physical CPU/GPU
    /// labels, so they remain "Acer Fan N" unless the kernel supplies a label.
    pub fn display_name(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        let chip = if self.chip.eq_ignore_ascii_case("acer") {
            "Acer".to_string()
        } else {
            self.chip.clone()
        };
        match self.role {
            SensorRole::FanRpm(index) => format!("{chip} Fan {}", index + 1),
            _ => self.role.to_string(),
        }
    }

    pub fn display_value(&self) -> String {
        match (self.value, self.unit) {
            (Some(v), Unit::Celsius) => format!("{v:.1} °C"),
            (Some(v), Unit::Rpm) => format!("{v:.0} rpm"),
            (Some(v), Unit::Percent) => format!("{v:.1} %"),
            (Some(v), Unit::Watt) => format!("{v:.1} W"),
            (None, _) => "unavailable".to_string(),
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
            .filter(|s| s.is_usable_for_regulation(now, ttl_ms))
            .max_by_key(|s| s.health.trust)
    }

    /// Pick the best die reading for a GPU index.
    pub fn gpu_die(&self, index: u32, now: u64, ttl_ms: u64) -> Option<&SensorSample> {
        self.sensors
            .iter()
            .filter(|s| s.role == SensorRole::GpuDie(index))
            .filter(|s| s.is_usable_for_regulation(now, ttl_ms))
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

    /// Highest valid CPU core reading, if any. This is for display only and
    /// never substitutes for the package reading in policy decisions.
    pub fn max_core_c(&self) -> Option<f64> {
        self.sensors
            .iter()
            .filter(|s| matches!(s.role, SensorRole::CpuCore(_)) && s.value.is_some())
            .filter_map(|s| s.value)
            .reduce(f64::max)
    }

    pub fn fan_rpm(&self, index: u32) -> Option<&SensorSample> {
        self.by_role(&SensorRole::FanRpm(index))
    }

    /// RPM feedback that is valid, fresh, trusted and a physical measurement.
    pub fn fan_rpm_feedback(&self, index: u32, now: u64, ttl_ms: u64) -> Option<&SensorSample> {
        self.sensors
            .iter()
            .find(|s| s.role == SensorRole::FanRpm(index) && s.is_usable_for_feedback(now, ttl_ms))
    }

    /// All fan tachometer sensors, without implying CPU/GPU identity.
    pub fn rpm_sensors(&self) -> impl Iterator<Item = &SensorSample> {
        self.sensors
            .iter()
            .filter(|s| matches!(s.role, SensorRole::FanRpm(_)))
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

#[cfg(test)]
mod trust_label_tests {
    use super::Trust;

    #[test]
    fn trust_short_labels_are_compact() {
        assert_eq!(Trust::Provisional.short_label(), "prov.");
        assert_eq!(Trust::Verified.short_label(), "verified");
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;

    fn sample(
        role: SensorRole,
        kind: SensorKind,
        value: Option<f64>,
        provider: SensorProvider,
    ) -> SensorSample {
        SensorSample::new(
            format!("test:{role}"),
            role,
            kind,
            value,
            match kind {
                SensorKind::Rpm => Unit::Rpm,
                SensorKind::Temperature => Unit::Celsius,
                _ => Unit::Percent,
            },
            value.map(|value| value.to_string()),
            match kind {
                SensorKind::Rpm => "rpm",
                SensorKind::Temperature => "millidegree_c",
                _ => "unit",
            },
            provider,
            "acer".to_string(),
            Some("/sys/class/hwmon/hwmon6/fan1_input".into()),
            None,
            Some("/sys/devices/platform/acer-wmi".into()),
        )
    }

    #[test]
    fn legacy_health_json_deserializes_with_defaults() {
        let health: SensorHealth = serde_json::from_str(
            r#"{"detected":true,"readable":true,"valid":true,"fresh":true,"trust":"verified","usable_for_control":true}"#,
        )
        .unwrap();
        assert!(health.usable_for_control);
        assert!(!health.usable_for_regulation);
        assert!(!health.usable_for_feedback);
        assert!(!health.physical_measurement);
    }

    #[test]
    fn rpm_is_feedback_not_regulation() {
        let sensor = sample(
            SensorRole::FanRpm(0),
            SensorKind::Rpm,
            Some(3488.0),
            SensorProvider::Hwmon,
        );
        assert!(sensor.health.physical_measurement);
        assert!(sensor.health.usable_for_feedback);
        assert!(!sensor.health.usable_for_regulation);
        assert!(!sensor.health.usable_for_control);
        assert_eq!(sensor.display_name(), "Acer Fan 1");
        let now = sensor.sampled_at_ms;
        assert!(sensor.is_usable_for_feedback(now, 3_000));
        assert!(!sensor.is_usable_for_regulation(now, 3_000));
    }

    #[test]
    fn temperature_is_regulation_not_feedback() {
        let sensor = sample(
            SensorRole::CpuPackage,
            SensorKind::Temperature,
            Some(55.0),
            SensorProvider::Hwmon,
        );
        assert!(sensor.health.physical_measurement);
        assert!(sensor.health.usable_for_regulation);
        assert!(sensor.health.usable_for_control);
        assert!(!sensor.health.usable_for_feedback);
    }

    #[test]
    fn zero_rpm_is_a_valid_observation_not_missing() {
        let sensor = sample(
            SensorRole::FanRpm(0),
            SensorKind::Rpm,
            Some(0.0),
            SensorProvider::Hwmon,
        );
        assert_eq!(sensor.value, Some(0.0));
        assert_eq!(sensor.status, SensorStatus::Ok);
        assert!(sensor.is_usable_for_feedback(sensor.sampled_at_ms, 3_000));
    }

    #[test]
    fn stale_rpm_is_not_feedback_eligible() {
        let sensor = sample(
            SensorRole::FanRpm(0),
            SensorKind::Rpm,
            Some(3488.0),
            SensorProvider::Hwmon,
        );
        let later = sensor.sampled_at_ms + 3_001;
        assert!(!sensor.is_usable_for_feedback(later, 3_000));
    }

    #[test]
    fn rpm_only_snapshot_cannot_satisfy_temperature_regulation() {
        let sensor = sample(
            SensorRole::FanRpm(0),
            SensorKind::Rpm,
            Some(3488.0),
            SensorProvider::Hwmon,
        );
        let snapshot = SensorSnapshot {
            sensors: vec![sensor],
            errors: Vec::new(),
            collected_at_ms: now_ms(),
        };
        let now = now_ms();
        assert!(snapshot.cpu_package(now, 3_000).is_none());
        assert!(snapshot.gpu_die(0, now, 3_000).is_none());
        assert_eq!(snapshot.rpm_sensors().count(), 1);
    }

    #[test]
    fn cpu_package_prefers_the_most_trusted_fresh_candidate() {
        let trusted = sample(
            SensorRole::CpuPackage,
            SensorKind::Temperature,
            Some(60.0),
            SensorProvider::Proc,
        )
        .with_trust(Trust::Trusted);
        let verified = sample(
            SensorRole::CpuPackage,
            SensorKind::Temperature,
            Some(55.0),
            SensorProvider::Hwmon,
        );
        let snapshot = SensorSnapshot {
            sensors: vec![trusted, verified],
            errors: Vec::new(),
            collected_at_ms: now_ms(),
        };
        let selected = snapshot.cpu_package(now_ms(), 3_000).unwrap();
        assert_eq!(selected.value, Some(55.0));
        assert_eq!(selected.health.trust, Trust::Verified);
    }

    #[test]
    fn gpu_source_selection_is_index_specific() {
        let gpu0 = sample(
            SensorRole::GpuDie(0),
            SensorKind::Temperature,
            Some(50.0),
            SensorProvider::Nvidia,
        );
        let gpu1 = sample(
            SensorRole::GpuDie(1),
            SensorKind::Temperature,
            Some(70.0),
            SensorProvider::Nvidia,
        );
        let snapshot = SensorSnapshot {
            sensors: vec![gpu0, gpu1],
            errors: Vec::new(),
            collected_at_ms: now_ms(),
        };
        assert_eq!(
            snapshot.gpu_die(0, now_ms(), 3_000).unwrap().value,
            Some(50.0)
        );
        assert_eq!(
            snapshot.gpu_die(1, now_ms(), 3_000).unwrap().value,
            Some(70.0)
        );
        assert!(snapshot.gpu_die(2, now_ms(), 3_000).is_none());
        assert_eq!(snapshot.gpu_indexes(), vec![0, 1]);
    }

    #[test]
    fn one_provider_failure_does_not_invalidate_unrelated_trusted_sensors() {
        let cpu = sample(
            SensorRole::CpuPackage,
            SensorKind::Temperature,
            Some(55.0),
            SensorProvider::Hwmon,
        );
        let snapshot = SensorSnapshot {
            sensors: vec![cpu],
            errors: vec![ProviderError {
                provider: SensorProvider::Nvidia,
                message: "nvidia-smi unavailable".into(),
            }],
            collected_at_ms: now_ms(),
        };
        assert!(snapshot.cpu_package(now_ms(), 3_000).is_some());
        assert!(snapshot.gpu_die(0, now_ms(), 3_000).is_none());
        assert_eq!(snapshot.errors.len(), 1);
    }

    #[test]
    fn unavailable_physical_sensor_keeps_provenance() {
        let mut sensor = sample(
            SensorRole::FanRpm(0),
            SensorKind::Rpm,
            Some(3488.0),
            SensorProvider::Hwmon,
        );
        sensor.mark_unavailable(Trust::Verified);
        assert!(sensor.health.physical_measurement);
        assert!(!sensor.health.usable_for_feedback);
        assert!(!sensor.health.usable_for_regulation);
    }

    #[test]
    fn downgraded_trust_removes_feedback_eligibility() {
        let sensor = sample(
            SensorRole::FanRpm(0),
            SensorKind::Rpm,
            Some(3488.0),
            SensorProvider::Hwmon,
        )
        .with_trust(Trust::Provisional);
        assert!(!sensor.health.usable_for_feedback);
        assert!(!sensor.health.usable_for_regulation);
    }
}
