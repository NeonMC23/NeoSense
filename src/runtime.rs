//! Shared read-only runtime view used by the CLI and TUI.
//!
//! This module never writes fan state. It collects sensors and reads NBFC state
//! through the same backend used by the supervisor.

use crate::model::SensorSnapshot;
use crate::nbfc::{NbfcCapabilities, NbfcCli, NbfcSensorConfig, NbfcStatus, NbfcVersion};
use crate::policy::fan_role;
use crate::regulation::{FanObservation, mapping_matches};
use crate::sensors::{SysfsRoot, collect_all};
use crate::service::{ServiceStatus, nbfc_service_status};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct HostInfo {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub bios: Option<String>,
    pub kernel: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeView {
    pub snapshot: SensorSnapshot,
    pub version: Option<NbfcVersion>,
    pub capabilities: Option<NbfcCapabilities>,
    pub status: Option<NbfcStatus>,
    pub service: ServiceStatus,
    pub sensor_config: Option<NbfcSensorConfig>,
    pub backend_error: Option<String>,
}

pub fn read_view() -> RuntimeView {
    let backend = NbfcCli::default();
    let snapshot = collect_all(&SysfsRoot::default());
    let version = backend.version().ok();
    let capabilities = version.map(NbfcCapabilities::from_version_with_socket);
    let (status, backend_error) = match (version, &capabilities) {
        (Some(version), Some(_)) => match backend.status(version) {
            Ok(status) => (Some(status), None),
            Err(e) => (None, Some(e)),
        },
        _ => (None, Some("NBFC unavailable".to_string())),
    };
    RuntimeView {
        snapshot,
        version,
        capabilities,
        status,
        service: nbfc_service_status(),
        sensor_config: backend.sensors_show().ok(),
        backend_error,
    }
}

pub fn fan_observations(view: &RuntimeView) -> Vec<FanObservation> {
    let Some(status) = &view.status else {
        return Vec::new();
    };
    status
        .fans
        .iter()
        .map(|fan| {
            let role = fan_role(&fan.name);
            let source = view
                .sensor_config
                .as_ref()
                .and_then(|config| config.fans.iter().find(|f| f.index == fan.index));
            let mapping_ok = source
                .map(|source| mapping_matches(role, &source.sensors))
                .unwrap_or(false);
            FanObservation {
                fan_index: fan.index,
                fan_name: fan.name.clone(),
                role,
                mapping_ok,
                // Local previews have no previous policy target. The current
                // NBFC target may be an automatic-curve value and must not
                // distort the reference curve.
                previous_target_percent: None,
                previous_temperature_c: None,
            }
        })
        .collect()
}

pub fn host_info() -> HostInfo {
    HostInfo {
        vendor: read_dmi("sys_vendor"),
        model: read_dmi("product_name"),
        bios: read_dmi("bios_version"),
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_string()),
    }
}

fn read_dmi(name: &str) -> Option<String> {
    std::fs::read_to_string(Path::new("/sys/class/dmi/id").join(name))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
