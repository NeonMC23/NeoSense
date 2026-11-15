use crate::custom_profiles::{CustomProfile, CustomStatus, Store as CustomStore, StoreError};
use crate::ipc::{ApiError, Request, Response, read_message, write_message};
use crate::model::{SensorSnapshot, now_ms};
use crate::nbfc::{FanBackend, NbfcCapabilities, NbfcSensorConfig, NbfcStatus, NbfcVersion};
use crate::policy::{
    FanRole, ManualLease, PolicyDecision, SafetyConfig, SafetyReport, SafetyState,
    assess_manual_request, fan_role, temperature_mapping,
};
use crate::regulation::{FanObservation, ProfileState, RegulationMode, evaluate_profile};
use crate::sensors::{SysfsRoot, collect_all};
use crate::service::{ServiceStatus, nbfc_service_status};
use crate::systemd::{Notifier, SystemdNotifier};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub socket: PathBuf,
    pub interval_ms: u64,
    pub safety: SafetyConfig,
    /// NBFC control socket used for read-only access-control classification.
    pub nbfc_socket: PathBuf,
    /// Persistent user curve store owned (and writable) by the supervisor.
    pub custom_profiles_path: PathBuf,
    /// If NBFC starts in a fixed state, return it to auto at supervisor start.
    pub startup_safe_auto: bool,
    /// Maximum concurrent IPC handler threads. A local group member must not
    /// be able to exhaust process threads by opening sockets indefinitely.
    pub max_connections: usize,
}

fn default_nbfc_socket() -> PathBuf {
    #[cfg(test)]
    {
        crate::nbfc::test_secure_socket_path()
    }
    #[cfg(not(test))]
    {
        crate::nbfc::default_socket_path()
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket: PathBuf::from("/run/neosense/neosensed.sock"),
            interval_ms: 2_000,
            safety: SafetyConfig::default(),
            nbfc_socket: default_nbfc_socket(),
            custom_profiles_path: crate::custom_profiles::system_path(),
            startup_safe_auto: true,
            max_connections: 32,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub generated_at_ms: u64,
    pub profile: ProfileState,
    pub nbfc_version: Option<String>,
    pub capabilities: Option<NbfcCapabilities>,
    pub nbfc: Option<NbfcStatus>,
    pub service: ServiceStatus,
    pub sensor_config: Option<NbfcSensorConfig>,
    pub sensors: SensorSnapshot,
    pub safety: SafetyReport,
    pub health: DaemonHealth,
    pub custom: CustomStatus,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResult {
    pub action: String,
    pub fan_index: Option<u32>,
    pub requested_percent: Option<f64>,
    pub target_percent: Option<f64>,
    pub reported_percent: Option<f64>,
    pub readback_plausible: Option<bool>,
    pub verified: bool,
    pub dry_run: bool,
    pub message: String,
    pub safety: SafetyReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonHealth {
    pub healthy: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManualSet {
    fan: u32,
    percent: f64,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct PresetSet {
    name: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Clone)]
struct RecoveryOutcome {
    verified: bool,
    error: Option<String>,
}

impl RecoveryOutcome {
    fn sentence(&self) -> String {
        if self.verified {
            "automatic mode confirmed by backend".to_string()
        } else {
            format!(
                "automatic recovery failed: {}",
                self.error.as_deref().unwrap_or("unknown error")
            )
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ApplyProfile {
    name: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ManualTarget {
    fan: u32,
    percent: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct MultiSet {
    targets: Vec<ManualTarget>,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ManualEnter {
    #[serde(default)]
    fan: Option<u32>,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct CustomName {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CustomDelete {
    name: String,
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct CustomRename {
    from: String,
    to: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CustomProfileBody {
    profile: CustomProfile,
}

#[derive(Debug, Clone, Deserialize)]
struct CustomDuplicate {
    from: String,
    to: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CustomReset {
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct HardwareValidation {
    name: String,
}

/// Classify a read-only NBFC analysis failure. The optional `acpiexec` tool is
/// not part of NeoSense; its absence must be actionable and must never look
/// like a fan-control failure.
fn hardware_analysis_error(error: String) -> ApiError {
    let lower = error.to_ascii_lowercase();
    if lower.contains("acpiexec") || lower.contains("no such file or directory") {
        ApiError::new(
            "analysis_unavailable",
            format!(
                "read-only configuration analysis is unavailable: {error}. NBFC rate-config requires acpiexec (acpica-tools); no fan write was performed. Use `hardware identity` and `diagnose` for the available evidence."
            ),
        )
        .with_details(vec![
            "acpica-tools is optional; install it separately only if you need NBFC rate-config analysis."
                .to_string(),
            "This failure does not affect automatic fan control or the supervisor.".to_string(),
        ])
    } else if lower.contains("root") {
        ApiError::new("root_required", error)
    } else {
        ApiError::new("backend_unavailable", error)
    }
}

struct State {
    nbfc_version: Option<NbfcVersion>,
    /// `nbfc_service` MainPID at the time `nbfc_version` was probed. A restart
    /// forces a version re-probe so an in-place NBFC upgrade cannot leave the
    /// supervisor with stale capabilities.
    nbfc_version_pid: Option<u32>,
    nbfc_version_probed_at_ms: u64,
    capabilities: Option<NbfcCapabilities>,
    nbfc: Option<NbfcStatus>,
    service: ServiceStatus,
    sensor_config: Option<NbfcSensorConfig>,
    sensor_config_refreshed_at_ms: u64,
    sensors: SensorSnapshot,
    profile: ProfileState,
    safety: SafetyReport,
    custom: CustomStatus,
    last_error: Option<String>,
}

pub struct Daemon {
    backend: Arc<dyn FanBackend>,
    root: SysfsRoot,
    config: DaemonConfig,
    state: Mutex<State>,
    /// Serializes policy transitions and sampler writes so an in-flight
    /// regulation iteration cannot overwrite a newer manual, preset,
    /// automatic or profile request.
    control: Mutex<()>,
}

impl Daemon {
    pub fn new(backend: Arc<dyn FanBackend>, root: SysfsRoot, config: DaemonConfig) -> Self {
        let custom = CustomStore::new(config.custom_profiles_path.clone()).status();
        let daemon = Self {
            backend,
            root,
            config,
            control: Mutex::new(()),
            state: Mutex::new(State {
                nbfc_version: None,
                nbfc_version_pid: None,
                nbfc_version_probed_at_ms: 0,
                capabilities: None,
                nbfc: None,
                service: ServiceStatus::default(),
                sensor_config: None,
                sensor_config_refreshed_at_ms: 0,
                sensors: SensorSnapshot::empty(),
                profile: ProfileState::default(),
                safety: SafetyReport::safe_auto(),
                custom,
                last_error: None,
            }),
        };
        daemon.refresh();
        daemon
    }

    pub fn refresh(&self) {
        const CONFIG_TTL_MS: u64 = 30_000;
        const VERSION_TTL_MS: u64 = 60_000;
        let now = now_ms();
        let service = nbfc_service_status();
        let (
            cached_version,
            cached_version_pid,
            version_probed_at_ms,
            cached_config,
            config_age,
            old_selected,
        ) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.nbfc_version,
                state.nbfc_version_pid,
                state.nbfc_version_probed_at_ms,
                state.sensor_config.clone(),
                now.saturating_sub(state.sensor_config_refreshed_at_ms),
                state
                    .nbfc
                    .as_ref()
                    .and_then(|status| status.selected_config.clone()),
            )
        };

        // The installed client version can change while this supervisor runs:
        // an NBFC upgrade replaces the client and restarts the service. Re-probe
        // when the service PID changes, the cached value ages out, or nothing
        // has been probed yet. This keeps `protected_manual` truthful instead
        // of pinning it to whatever was installed at supervisor startup.
        let version_stale = version_needs_refresh(
            now,
            cached_version,
            cached_version_pid,
            version_probed_at_ms,
            service.main_pid,
            VERSION_TTL_MS,
        );
        let mut version = cached_version;
        let mut version_pid = cached_version_pid;
        let mut version_probed_at_ms = version_probed_at_ms;
        if version_stale && let Ok(probed) = self.backend.version() {
            version = Some(probed);
            version_pid = service.main_pid;
            version_probed_at_ms = now;
        }

        let capabilities = version.map(|version| {
            NbfcCapabilities::from_version(version).with_socket_security(
                crate::nbfc::inspect_socket_with_identity(
                    &self.config.nbfc_socket,
                    &crate::nbfc::SupervisorIdentity::current(),
                ),
            )
        });
        let nbfc = match (version, &capabilities) {
            (Some(v), Some(_)) => self.backend.status(v).ok(),
            _ => None,
        };
        let selected_changed = old_selected
            != nbfc
                .as_ref()
                .and_then(|status| status.selected_config.clone());
        let refresh_config =
            cached_config.is_none() || selected_changed || config_age >= CONFIG_TTL_MS;
        let sensor_config = if refresh_config {
            self.backend.sensors_show().ok()
        } else {
            cached_config
        };
        let sensor_config_refreshed_at_ms = if refresh_config {
            now
        } else {
            now.saturating_sub(config_age)
        };
        let error = if version.is_none() {
            Some("NBFC unavailable".to_string())
        } else if nbfc.is_none() {
            Some("NBFC status unavailable".to_string())
        } else {
            None
        };

        let sensors = collect_all(&self.root);
        let mut state = self.state.lock().expect("poisoned mutex");
        // Evaluate the profile against the selection that is current at commit
        // time, not a snapshot taken before the slow backend calls. Otherwise a
        // concurrent profile.select can be overwritten by a stale default
        // state (lost update).
        let profile = evaluate_selected_profile(
            state.profile.selected.as_deref(),
            &sensors,
            nbfc.as_ref(),
            sensor_config.as_ref(),
            &state.profile,
            &state.custom.profiles,
            state.custom.error.as_deref(),
            now,
        );
        state.sensors = sensors;
        state.service = service;
        state.nbfc_version = version;
        state.nbfc_version_pid = version_pid;
        state.nbfc_version_probed_at_ms = version_probed_at_ms;
        state.capabilities = capabilities;
        state.nbfc = nbfc;
        state.sensor_config = sensor_config;
        state.sensor_config_refreshed_at_ms = sensor_config_refreshed_at_ms;
        state.profile = profile;
        state.last_error = error;
    }

    /// Called once at startup. It is the only place where the supervisor may
    /// write without a user request, and only to restore the safe automatic
    /// state.
    pub fn enforce_startup_safety(&self) -> Result<(), String> {
        let _control = self.lock_control();
        if !self.config.startup_safe_auto {
            return Ok(());
        }
        let fixed = self
            .state
            .lock()
            .expect("poisoned mutex")
            .nbfc
            .as_ref()
            .map(|s| s.fans.iter().any(|f| f.auto_mode == Some(false)))
            .unwrap_or(false);
        if fixed {
            self.auto_internal("fixed state detected at startup")
                .map(|_| ())
                .map_err(|e| e.message)
        } else {
            Ok(())
        }
    }

    pub fn status(&self) -> DaemonStatus {
        let state = self.state.lock().expect("poisoned mutex");
        DaemonStatus {
            generated_at_ms: now_ms(),
            profile: state.profile.clone(),
            nbfc_version: state.nbfc_version.map(|v| v.to_string()),
            capabilities: state.capabilities.clone(),
            nbfc: state.nbfc.clone(),
            service: state.service.clone(),
            sensor_config: state.sensor_config.clone(),
            sensors: state.sensors.clone(),
            safety: state.safety.clone(),
            health: health_from_state(&state, &self.config),
            custom: state.custom.clone(),
            last_error: state.last_error.clone(),
        }
    }

    /// A snapshot's age is part of health; a stalled sampler must eventually
    /// stop the watchdog heartbeat instead of pretending to be alive.
    pub fn health(&self) -> DaemonHealth {
        match self.state.lock() {
            Ok(state) => health_from_state(&state, &self.config),
            Err(_) => DaemonHealth {
                healthy: false,
                reason: Some("internal state poisoned".to_string()),
            },
        }
    }

    /// Reload the persistent custom-profile store into supervisor state.
    /// Corruption is reported and never replaced with defaults.
    pub fn reload_custom(&self) -> CustomStatus {
        let status = CustomStore::new(self.config.custom_profiles_path.clone()).status();
        self.state.lock().expect("poisoned mutex").custom = status.clone();
        self.refresh();
        status
    }

    fn custom_error(&self, error: StoreError) -> ApiError {
        ApiError::new(error.code(), error.to_string())
    }

    fn forget_selection(&self, name: &str) {
        let cleared = {
            let mut state = self.state.lock().expect("poisoned mutex");
            if state.profile.selected.as_deref() == Some(name) {
                state.profile = ProfileState::default();
                true
            } else {
                false
            }
        };
        if cleared {
            self.refresh();
        }
    }

    fn rename_selection(&self, from: &str, to: &str) {
        let renamed = {
            let mut state = self.state.lock().expect("poisoned mutex");
            if state.profile.selected.as_deref() == Some(from) {
                state.profile.selected = Some(to.to_string());
                true
            } else {
                false
            }
        };
        if renamed {
            self.refresh();
        }
    }

    fn clear_deleted_custom_selection(&self) {
        let should_clear = {
            let state = self.state.lock().expect("poisoned mutex");
            state
                .profile
                .selected
                .as_deref()
                .is_some_and(|name| crate::profiles::find(name).is_none())
        };
        if should_clear {
            self.state.lock().expect("poisoned mutex").profile = ProfileState::default();
            self.refresh();
        }
    }

    fn custom_after(
        &self,
        id: u64,
        result: Result<crate::custom_profiles::Loaded, StoreError>,
        action: &str,
    ) -> Response {
        match result {
            Ok(loaded) => {
                let status = self.reload_custom();
                Response::ok(
                    id,
                    serde_json::json!({
                        "schema": "neosense.custom_profile_result/v1",
                        "action": action,
                        "status": status,
                        "warnings": loaded.warnings,
                    }),
                )
            }
            Err(error) => Response::error(id, self.custom_error(error)),
        }
    }

    pub fn handle_request(&self, request: &Request) -> Response {
        match request.method.as_str() {
            "ping" => Response::ok(request.id, serde_json::json!({"pong": true})),
            "status.get" => {
                let _control = self.lock_control();
                Response::ok(request.id, self.status())
            }
            "profile.get" => {
                let _control = self.lock_control();
                let status = self.status();
                Response::ok(
                    request.id,
                    serde_json::json!({
                        "schema": "neosense.profile_state/v1",
                        "profile": status.profile,
                        "safety": status.safety,
                    }),
                )
            }
            "profile.select" => {
                let _control = self.lock_control();
                self.select_profile(request)
            }
            "profile.apply" => {
                let _control = self.lock_control();
                match serde_json::from_value::<ApplyProfile>(request.params.clone()) {
                    Ok(apply) => match self.apply_profile_internal(apply) {
                        Ok(result) => Response::ok(request.id, result),
                        Err(error) => Response::error(request.id, error),
                    },
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid profile.apply parameters: {error}"),
                        ),
                    ),
                }
            }
            "profile.clear" => {
                let _control = self.lock_control();
                let active = self.state.lock().expect("poisoned mutex").profile.active;
                if active {
                    match self.auto_internal("profile cleared by user") {
                        Ok(_) => {}
                        Err(error) => return Response::error(request.id, error),
                    }
                }
                self.state.lock().expect("poisoned mutex").profile = ProfileState::default();
                self.refresh();
                Response::ok(
                    request.id,
                    serde_json::json!({
                        "schema": "neosense.profile_state/v1",
                        "profile": self.status().profile,
                        "hardware_applied": false,
                        "was_active": active,
                        "message": if active {
                            "profile cleared; active regulation stopped and fans confirmed automatic"
                        } else {
                            "profile selection cleared; no fan write performed"
                        }
                    }),
                )
            }
            "custom.list" | "custom.reload" => {
                let status = self.reload_custom();
                Response::ok(request.id, status)
            }
            "custom.get" => match serde_json::from_value::<CustomName>(request.params.clone()) {
                Ok(query) => match CustomStore::new(self.config.custom_profiles_path.clone())
                    .load()
                    .and_then(|loaded| {
                        loaded
                            .file
                            .find(&query.name)
                            .cloned()
                            .ok_or_else(|| StoreError::NotFound(query.name.clone()))
                    }) {
                    Ok(profile) => Response::ok(
                        request.id,
                        serde_json::json!({
                            "schema": "neosense.custom_profile/v1",
                            "profile": profile,
                        }),
                    ),
                    Err(error) => Response::error(request.id, self.custom_error(error)),
                },
                Err(error) => Response::error(
                    request.id,
                    ApiError::new(
                        "bad_request",
                        format!("invalid custom.get parameters: {error}"),
                    ),
                ),
            },
            "custom.validate" => {
                match serde_json::from_value::<CustomProfileBody>(request.params.clone()) {
                    Ok(body) => match body.profile.validate() {
                        Ok(()) => Response::ok(
                            request.id,
                            serde_json::json!({
                                "schema": "neosense.custom_profile_validation/v1",
                                "valid": true,
                                "name": body.profile.name,
                            }),
                        ),
                        Err(error) => {
                            Response::error(request.id, ApiError::new("config_invalid", error))
                        }
                    },
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid custom.validate parameters: {error}"),
                        ),
                    ),
                }
            }
            "custom.create" => {
                match serde_json::from_value::<CustomProfileBody>(request.params.clone()) {
                    Ok(body) => {
                        let result = CustomStore::new(self.config.custom_profiles_path.clone())
                            .create(body.profile);
                        self.custom_after(request.id, result, "created")
                    }
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid custom.create parameters: {error}"),
                        ),
                    ),
                }
            }
            "custom.update" => {
                let _control = self.lock_control();
                match serde_json::from_value::<CustomProfileBody>(request.params.clone()) {
                    Ok(body) => {
                        if let Err(error) = body.profile.validate() {
                            return Response::error(
                                request.id,
                                ApiError::new("config_invalid", error),
                            );
                        }
                        if self.active_profile_is(&body.profile.name)
                            && let Err(error) =
                                self.auto_internal("active custom profile was edited")
                        {
                            return Response::error(request.id, error);
                        }
                        let result = CustomStore::new(self.config.custom_profiles_path.clone())
                            .update(body.profile);
                        self.custom_after(request.id, result, "updated")
                    }
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid custom.update parameters: {error}"),
                        ),
                    ),
                }
            }
            "custom.delete" => {
                let _control = self.lock_control();
                match serde_json::from_value::<CustomDelete>(request.params.clone()) {
                    Ok(body) => {
                        if !body.confirm {
                            Response::error(
                                request.id,
                                ApiError::new(
                                    "confirmation_required",
                                    "deleting a custom profile requires explicit confirmation",
                                ),
                            )
                        } else {
                            if self.active_profile_is(&body.name)
                                && let Err(error) =
                                    self.auto_internal("active custom profile was deleted")
                            {
                                return Response::error(request.id, error);
                            }
                            let result = CustomStore::new(self.config.custom_profiles_path.clone())
                                .delete(&body.name);
                            let response = self.custom_after(request.id, result, "deleted");
                            if response.ok {
                                self.forget_selection(&body.name);
                            }
                            response
                        }
                    }
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid custom.delete parameters: {error}"),
                        ),
                    ),
                }
            }
            "custom.duplicate" => {
                match serde_json::from_value::<CustomDuplicate>(request.params.clone()) {
                    Ok(body) => {
                        let result = CustomStore::new(self.config.custom_profiles_path.clone())
                            .duplicate(&body.from, &body.to);
                        self.custom_after(request.id, result, "duplicated")
                    }
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid custom.duplicate parameters: {error}"),
                        ),
                    ),
                }
            }
            "custom.rename" => {
                let _control = self.lock_control();
                match serde_json::from_value::<CustomRename>(request.params.clone()) {
                    Ok(body) => {
                        if self.active_profile_is(&body.from)
                            && let Err(error) =
                                self.auto_internal("active custom profile was renamed")
                        {
                            return Response::error(request.id, error);
                        }
                        let result = CustomStore::new(self.config.custom_profiles_path.clone())
                            .rename(&body.from, &body.to);
                        let response = self.custom_after(request.id, result, "renamed");
                        if response.ok {
                            self.rename_selection(&body.from, &body.to);
                        }
                        response
                    }
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid custom.rename parameters: {error}"),
                        ),
                    ),
                }
            }
            "custom.reset" => {
                let _control = self.lock_control();
                match serde_json::from_value::<CustomReset>(request.params.clone()) {
                    Ok(body) => {
                        if !body.confirm {
                            Response::error(
                                request.id,
                                ApiError::new(
                                    "confirmation_required",
                                    "resetting the custom profile store requires explicit confirmation",
                                ),
                            )
                        } else {
                            let active_custom =
                                {
                                    let state = self.state.lock().expect("poisoned mutex");
                                    state.profile.active
                                        && state.profile.selected.as_deref().is_some_and(|name| {
                                            crate::profiles::find(name).is_none()
                                        })
                                };
                            if active_custom
                                && let Err(error) =
                                    self.auto_internal("active custom profile store was reset")
                            {
                                return Response::error(request.id, error);
                            }
                            match CustomStore::new(self.config.custom_profiles_path.clone())
                                .reset(true)
                            {
                                Ok(preserved) => {
                                    let status = self.reload_custom();
                                    self.clear_deleted_custom_selection();
                                    Response::ok(
                                        request.id,
                                        serde_json::json!({
                                            "schema": "neosense.custom_profile_result/v1",
                                            "action": "reset",
                                            "status": status,
                                            "preserved": preserved.map(|path| path.display().to_string()),
                                        }),
                                    )
                                }
                                Err(error) => Response::error(request.id, self.custom_error(error)),
                            }
                        }
                    }
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid custom.reset parameters: {error}"),
                        ),
                    ),
                }
            }
            "hardware.validate" => {
                match serde_json::from_value::<HardwareValidation>(request.params.clone()) {
                    Ok(query) => match self.backend.rate_config_json(&query.name) {
                        Ok(analysis) => {
                            let analysis = serde_json::from_str::<serde_json::Value>(&analysis)
                                .unwrap_or(serde_json::Value::String(analysis));
                            Response::ok(
                                request.id,
                                serde_json::json!({
                                    "schema": "neosense.hardware_validation/v1",
                                    "name": query.name,
                                    "analysis": analysis,
                                    "hardware_applied": false,
                                }),
                            )
                        }
                        Err(error) => Response::error(request.id, hardware_analysis_error(error)),
                    },
                    Err(error) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid hardware.validate parameters: {error}"),
                        ),
                    ),
                }
            }
            "fan.auto" => {
                let _control = self.lock_control();
                let requested_fan = request
                    .params
                    .get("fan")
                    .and_then(serde_json::Value::as_u64)
                    .map(|value| value as u32);
                let result = match requested_fan {
                    Some(fan) => self.auto_fan_internal(fan, "user request"),
                    None => self.auto_internal("user request"),
                };
                match result {
                    Ok(result) => Response::ok(request.id, result),
                    Err(e) => Response::error(request.id, e),
                }
            }
            "fan.set" => {
                let _control = self.lock_control();
                match serde_json::from_value::<ManualSet>(request.params.clone()) {
                    Ok(set) => match self.manual_internal(set) {
                        Ok(result) => Response::ok(request.id, result),
                        Err(e) => Response::error(request.id, e),
                    },
                    Err(e) => Response::error(
                        request.id,
                        ApiError::new("bad_request", format!("invalid fan.set parameters: {e}")),
                    ),
                }
            }
            "fan.set_many" => {
                let _control = self.lock_control();
                match serde_json::from_value::<MultiSet>(request.params.clone()) {
                    Ok(set) => match self.set_many_internal(
                        "set_many",
                        set.targets
                            .into_iter()
                            .map(|target| (target.fan, target.percent))
                            .collect(),
                        set.confirm,
                        set.dry_run,
                    ) {
                        Ok(result) => Response::ok(request.id, result),
                        Err(e) => Response::error(request.id, e),
                    },
                    Err(e) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid fan.set_many parameters: {e}"),
                        ),
                    ),
                }
            }
            "fan.manual" => {
                let _control = self.lock_control();
                match serde_json::from_value::<ManualEnter>(request.params.clone()) {
                    Ok(set) => match self.manual_enter_internal(set) {
                        Ok(result) => Response::ok(request.id, result),
                        Err(e) => Response::error(request.id, e),
                    },
                    Err(e) => Response::error(
                        request.id,
                        ApiError::new("bad_request", format!("invalid fan.manual parameters: {e}")),
                    ),
                }
            }
            "preset.apply" => {
                let _control = self.lock_control();
                match serde_json::from_value::<PresetSet>(request.params.clone()) {
                    Ok(set) => match self.preset_internal(set) {
                        Ok(result) => Response::ok(request.id, result),
                        Err(e) => Response::error(request.id, e),
                    },
                    Err(e) => Response::error(
                        request.id,
                        ApiError::new(
                            "bad_request",
                            format!("invalid preset.apply parameters: {e}"),
                        ),
                    ),
                }
            }
            _ => Response::error(
                request.id,
                ApiError::new(
                    "unknown_method",
                    format!("unknown IPC method: {}", request.method),
                ),
            ),
        }
    }

    fn lock_control(&self) -> std::sync::MutexGuard<'_, ()> {
        self.control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn active_profile_is(&self, name: &str) -> bool {
        let state = self.state.lock().expect("poisoned mutex");
        state.profile.active && state.profile.selected.as_deref() == Some(name)
    }

    fn end_active_profile(&self, reason: &str) -> Result<(), ApiError> {
        let active = self.state.lock().expect("poisoned mutex").profile.active;
        if active {
            self.auto_internal(reason)?;
        }
        Ok(())
    }

    fn apply_profile_internal(&self, request: ApplyProfile) -> Result<serde_json::Value, ApiError> {
        if !request.confirm {
            return Err(ApiError::new(
                "confirmation_required",
                "profile application requires explicit confirmation",
            ));
        }
        let custom = self.state.lock().expect("poisoned mutex").custom.clone();
        let Some(profile) = crate::profiles::resolve(&request.name, &custom.profiles) else {
            return Err(ApiError::new(
                "bad_request",
                format!("unknown profile: {}", request.name),
            ));
        };
        if profile.action == crate::profiles::ProfileAction::SafeAuto {
            if request.dry_run {
                return Ok(serde_json::json!({
                    "schema": "neosense.profile_apply/v1",
                    "profile": request.name,
                    "action": "safe_auto",
                    "hardware_applied": false,
                    "verified": false,
                    "partial": false,
                    "dry_run": true,
                    "applied_fans": [],
                    "blocked_fans": [],
                    "message": "valid safe-auto request; no write performed (dry-run)",
                }));
            }
            let result = self.auto_internal("safe-auto profile requested")?;
            return Ok(serde_json::json!({
                "schema": "neosense.profile_apply/v1",
                "profile": request.name,
                "action": "safe_auto",
                "hardware_applied": true,
                "verified": result.verified,
                "partial": false,
                "applied_fans": [],
                "blocked_fans": [],
                "message": "all fans returned to NBFC automatic mode",
            }));
        }
        let Some(curve) = profile.curve.clone() else {
            return Err(ApiError::new(
                "unsupported_profile",
                format!("profile {} has no regulation curve", request.name),
            ));
        };
        let capabilities = self.live_capabilities();
        let (snapshot, status, sensor_config, safety_config) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.sensors.clone(),
                state.nbfc.clone(),
                state.sensor_config.clone(),
                self.config.safety.clone(),
            )
        };
        let Some(status) = status else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC status unavailable; profile not applied",
            ));
        };
        let Some(capabilities) = capabilities else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC capabilities unavailable; profile not applied",
            ));
        };
        let now = now_ms();
        let selected_roles = profile.fan_roles.as_ref();
        let fans: Vec<FanObservation> = status
            .fans
            .iter()
            .filter(|fan| selected_roles.is_none_or(|roles| roles.contains(&fan_role(&fan.name))))
            .map(|fan| {
                let role = fan_role(&fan.name);
                let source = sensor_config
                    .as_ref()
                    .and_then(|config| config.fans.iter().find(|source| source.index == fan.index));
                FanObservation {
                    fan_index: fan.index,
                    fan_name: fan.name.clone(),
                    role,
                    mapping_ok: temperature_mapping(
                        role,
                        source,
                        &snapshot,
                        now,
                        safety_config.stale_after_ms,
                    )
                    .matched,
                    previous_target_percent: None,
                    previous_temperature_c: None,
                }
            })
            .collect();
        if fans.is_empty() {
            return Err(ApiError::new(
                "unsupported_profile",
                "no backend fan matches the profile's selected fan roles",
            ));
        }
        let decisions = evaluate_profile(&curve, &snapshot, &fans, RegulationMode::Active, now)
            .map_err(|error| ApiError::new("unsupported_profile", error))?;
        let mut eligible = Vec::new();
        let mut blocked = Vec::new();
        for decision in &decisions {
            let probe = decision
                .target_percent
                .unwrap_or_else(|| 50.0_f64.clamp(safety_config.min_percent as f64, 100.0));
            let policy = assess_manual_request(
                &snapshot,
                &status,
                &capabilities,
                sensor_config.as_ref(),
                decision.fan_index,
                probe,
                &safety_config,
                now,
            );
            let Some(target) = decision.target_percent else {
                let mut reasons = vec![
                    decision
                        .reason
                        .clone()
                        .unwrap_or_else(|| "no target available".to_string()),
                ];
                for reason in policy.reasons {
                    if !reasons.contains(&reason) {
                        reasons.push(reason);
                    }
                }
                blocked.push((decision.fan_index, decision.fan_name.clone(), reasons));
                continue;
            };
            if policy.allowed {
                eligible.push((decision.fan_index, decision.fan_name.clone(), target));
            } else {
                blocked.push((
                    decision.fan_index,
                    decision.fan_name.clone(),
                    policy.reasons,
                ));
            }
        }
        if eligible.is_empty() {
            return Err(ApiError::new(
                "unsafe_refused",
                "no fan is eligible for this profile; no writes performed",
            )
            .with_details(
                blocked
                    .iter()
                    .flat_map(|(_, name, reasons)| {
                        reasons
                            .iter()
                            .map(move |reason| format!("{name}: {reason}"))
                    })
                    .collect(),
            ));
        }
        if request.dry_run {
            return Ok(serde_json::json!({
                "schema": "neosense.profile_apply/v1",
                "profile": request.name,
                "action": "closed_loop",
                "hardware_applied": false,
                "verified": false,
                "partial": !blocked.is_empty(),
                "dry_run": true,
                "applied_fans": eligible.iter().map(|(index, _, _)| index).collect::<Vec<_>>(),
                "blocked_fans": blocked.iter().map(|(index, _, _)| index).collect::<Vec<_>>(),
                "blocked": blocked.iter().map(|(index, name, reasons)| serde_json::json!({
                    "fan_index": index, "fan_name": name, "reasons": reasons
                })).collect::<Vec<_>>(),
                "targets": decisions,
                "message": "valid profile activation; no write performed (dry-run)",
            }));
        }
        // Acquire ownership of every fan before writing a new target. This
        // returns an active profile or any independent manual/preset fans to
        // automatic, so no fan can remain manually controlled without a lease
        // while the profile reports itself active.
        self.auto_internal("profile transition")?;
        let mut commands = Vec::new();
        for (index, name, target) in &eligible {
            let result = match self.backend.set_speed(*index, *target) {
                Ok(output) => (
                    output.success,
                    if output.success {
                        None
                    } else {
                        Some(output.combined_error())
                    },
                ),
                Err(error) => (false, Some(error)),
            };
            if result.0 {
                commands.push((*index, name.clone(), *target, None));
            } else {
                commands.push((*index, name.clone(), *target, result.1));
            }
        }
        self.refresh();
        let (post, post_snapshot, post_sensor_config) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.nbfc.clone(),
                state.sensors.clone(),
                state.sensor_config.clone(),
            )
        };
        let mut verified_fans = Vec::new();
        let mut failures = Vec::new();
        if let Some(post) = &post {
            for (index, name, target, command_error) in &commands {
                let fan = post.fan(*index);
                let target_ok = fan
                    .and_then(|fan| fan.target_speed)
                    .map(|value| (value - target).abs() <= 1.0)
                    .unwrap_or(false);
                let mode_ok = fan.and_then(|fan| fan.auto_mode) == Some(false);
                let post_policy = assess_manual_request(
                    &post_snapshot,
                    post,
                    &capabilities,
                    post_sensor_config.as_ref(),
                    *index,
                    *target,
                    &safety_config,
                    now_ms(),
                );
                if command_error.is_none() && target_ok && mode_ok && post_policy.allowed {
                    verified_fans.push(*index);
                } else {
                    failures.push(serde_json::json!({
                        "fan_index": index,
                        "fan_name": name,
                        "requested_percent": target,
                        "error": command_error,
                        "target_percent": fan.and_then(|fan| fan.target_speed),
                        "mode_ok": mode_ok,
                        "post_policy_reasons": post_policy.reasons,
                    }));
                }
            }
        } else {
            failures
                .push(serde_json::json!({"error": "NBFC status unavailable after profile writes"}));
        }
        if !failures.is_empty() || verified_fans.len() != eligible.len() {
            let recovery = self.fail_safe_after_control_failure(&format!(
                "profile {} activation failed for {} fan(s)",
                request.name,
                eligible.len()
            ));
            self.mark_profile_inactive(&format!("profile {} activation failed", request.name));
            return Ok(serde_json::json!({
                "schema": "neosense.profile_apply/v1",
                "profile": request.name,
                "action": "closed_loop",
                "hardware_applied": false,
                "verified": false,
                "partial": false,
                "automatic_recovery": recovery.verified,
                "recovery_failed": !recovery.verified,
                "recovery_error": recovery.error,
                "applied_fans": [],
                "verified_before_recovery": verified_fans,
                "blocked_fans": blocked.iter().map(|(index, _, _)| index).collect::<Vec<_>>(),
                "failures": failures,
                "targets": decisions,
                "message": format!(
                    "profile activation failed; {}",
                    recovery.sentence()
                ),
            }));
        }
        let mut state = self.state.lock().expect("poisoned mutex");
        state.profile = ProfileState {
            selected: Some(request.name.clone()),
            mode: Some(RegulationMode::Active),
            active: true,
            applied_fans: verified_fans.clone(),
            blocked_fans: blocked.iter().map(|(index, _, _)| *index).collect(),
            evaluated_at_ms: now,
            targets: decisions,
            reason: Some("active profile; supervisor closed-loop regulation".to_string()),
        };
        let safety = SafetyReport {
            state: SafetyState::Profile,
            reasons: blocked
                .iter()
                .map(|(index, name, reasons)| {
                    format!("fan {index} ({name}) blocked: {}", reasons.join("; "))
                })
                .collect(),
            manual: None,
            preset: None,
            updated_at_ms: now,
        };
        state.safety = safety.clone();
        Ok(serde_json::json!({
            "schema": "neosense.profile_apply/v1",
            "profile": request.name,
            "action": "closed_loop",
            "hardware_applied": true,
            "verified": true,
            "partial": !blocked.is_empty(),
            "dry_run": false,
            "applied_fans": verified_fans,
            "blocked_fans": blocked.iter().map(|(index, _, _)| index).collect::<Vec<_>>(),
            "blocked": blocked.iter().map(|(index, name, reasons)| serde_json::json!({
                "fan_index": index, "fan_name": name, "reasons": reasons
            })).collect::<Vec<_>>(),
            "targets": state.profile.targets,
            "safety": safety,
            "message": "profile active; supervisor will regulate eligible fans from fresh telemetry",
        }))
    }

    fn regulate_profile_once(&self) -> Result<(), String> {
        let (active, applied, targets, status, profile_name) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.profile.active,
                state.profile.applied_fans.clone(),
                state.profile.targets.clone(),
                state.nbfc.clone(),
                state.profile.selected.clone().unwrap_or_default(),
            )
        };
        if !active {
            return Ok(());
        }
        let Some(status) = status else {
            return Err(format!("profile {profile_name}: NBFC status unavailable"));
        };
        if applied.is_empty() {
            return Err(format!("profile {profile_name}: no applied fans"));
        }
        let mut writes = Vec::new();
        for index in &applied {
            let Some(decision) = targets.iter().find(|target| target.fan_index == *index) else {
                return Err(format!("profile {profile_name}: fan {index} has no target"));
            };
            if !decision.available {
                return Err(format!(
                    "profile {profile_name}: fan {index} telemetry unavailable: {}",
                    decision
                        .reason
                        .clone()
                        .unwrap_or_else(|| "no fresh trusted source".into())
                ));
            }
            let Some(target) = decision.target_percent else {
                return Err(format!("profile {profile_name}: fan {index} has no target"));
            };
            let Some(fan) = status.fan(*index) else {
                return Err(format!(
                    "profile {profile_name}: fan {index} disappeared from the backend"
                ));
            };
            let current = fan.target_speed;
            if current
                .map(|value| (value - target).abs() < 1.0)
                .unwrap_or(false)
                && fan.auto_mode == Some(false)
            {
                continue;
            }
            writes.push((*index, target));
        }
        for (index, target) in &writes {
            let output = self
                .backend
                .set_speed(*index, *target)
                .map_err(|error| format!("fan {index} write failed: {error}"))?;
            if !output.success {
                return Err(format!(
                    "fan {index} write failed: {}",
                    output.combined_error()
                ));
            }
        }
        if writes.is_empty() {
            return Ok(());
        }
        self.refresh();
        let post = self.state.lock().expect("poisoned mutex").nbfc.clone();
        let Some(post) = post else {
            return Err(format!(
                "profile {profile_name}: NBFC status unavailable after write"
            ));
        };
        for (index, target) in &writes {
            let fan = post.fan(*index);
            let target_ok = fan
                .and_then(|fan| fan.target_speed)
                .map(|value| (value - target).abs() <= 1.0)
                .unwrap_or(false);
            let mode_ok = fan.and_then(|fan| fan.auto_mode) == Some(false);
            if !target_ok || !mode_ok {
                return Err(format!(
                    "profile {profile_name}: fan {index} target/mode not confirmed"
                ));
            }
        }
        Ok(())
    }

    fn select_profile(&self, request: &Request) -> Response {
        if self.state.lock().expect("poisoned mutex").profile.active {
            return Response::error(
                request.id,
                ApiError::new(
                    "profile_active",
                    "an active profile is running; deactivate it before selecting a simulation profile",
                ),
            );
        }
        let name = request
            .params
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let custom = self.state.lock().expect("poisoned mutex").custom.clone();
        let Some(profile) = crate::profiles::resolve(name, &custom.profiles) else {
            if let Some(error) = custom.error {
                return Response::error(
                    request.id,
                    ApiError::new(
                        "config_corrupt",
                        format!("custom profile store unavailable: {error}"),
                    ),
                );
            }
            return Response::error(
                request.id,
                ApiError::new("bad_request", format!("unknown profile: {name}")),
            );
        };
        if profile.curve.is_none() {
            return Response::error(
                request.id,
                ApiError::new(
                    "unsupported_profile",
                    format!("profile {name} has no validated regulation curve"),
                ),
            );
        }
        {
            let mut state = self.state.lock().expect("poisoned mutex");
            state.profile.selected = Some(name.to_string());
            state.profile.mode = Some(RegulationMode::Simulated);
        }
        self.refresh();
        Response::ok(
            request.id,
            serde_json::json!({
                "schema": "neosense.profile_state/v1",
                "profile": self.status().profile,
                "hardware_applied": false,
                "message": "profile selected for simulation; no fan write performed"
            }),
        )
    }

    /// Re-inspect the NBFC version and socket immediately before a protected
    /// operation. Neither the cached refresh result nor a version cached before
    /// an NBFC upgrade is trusted at a write gate; if the client cannot report
    /// its version, the operation fails closed.
    fn live_capabilities(&self) -> Option<NbfcCapabilities> {
        let version = self.backend.version().ok()?;
        let capabilities = NbfcCapabilities::from_version(version).with_socket_security(
            crate::nbfc::inspect_socket_with_identity(
                &self.config.nbfc_socket,
                &crate::nbfc::SupervisorIdentity::current(),
            ),
        );
        let mut state = self.state.lock().expect("poisoned mutex");
        state.nbfc_version = Some(version);
        state.capabilities = Some(capabilities.clone());
        Some(capabilities)
    }

    fn preset_internal(&self, set: PresetSet) -> Result<serde_json::Value, ApiError> {
        if !set.confirm {
            return Err(ApiError::new(
                "confirmation_required",
                "fixed preset application requires explicit confirmation",
            ));
        }
        let Some(preset) = crate::presets::find(&set.name) else {
            return Err(ApiError::new(
                "bad_request",
                format!("unknown preset: {}", set.name),
            ));
        };
        let capabilities = self.live_capabilities();
        let (snapshot, status, sensor_config, safety_config) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.sensors.clone(),
                state.nbfc.clone(),
                state.sensor_config.clone(),
                self.config.safety.clone(),
            )
        };
        let Some(status) = status else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC status unavailable; preset refused",
            ));
        };
        let Some(capabilities) = capabilities else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC capabilities unavailable; preset refused",
            ));
        };
        let mut plans = Vec::new();
        for fan in &status.fans {
            let role = fan_role(&fan.name);
            let target = match role {
                FanRole::Cpu => preset.cpu_percent,
                FanRole::Gpu => preset.gpu_percent,
                FanRole::Unknown => {
                    return Err(ApiError::new(
                        "unsafe_refused",
                        format!("fan {} has an ambiguous role; preset refused", fan.index),
                    ));
                }
            };
            let decision = assess_manual_request(
                &snapshot,
                &status,
                &capabilities,
                sensor_config.as_ref(),
                fan.index,
                target,
                &safety_config,
                now_ms(),
            );
            if !decision.allowed {
                return Err(ApiError::new(
                    "unsafe_refused",
                    format!("preset {} refused before any write", preset.name),
                )
                .with_details(decision.reasons));
            }
            plans.push((fan.index, fan.name.clone(), target));
        }
        if plans.is_empty() {
            return Err(ApiError::new(
                "backend_unavailable",
                "no NBFC fan is available",
            ));
        }
        if set.dry_run {
            return Ok(serde_json::json!({
                "schema": "neosense.preset_result/v1",
                "preset": preset.name,
                "verified": false,
                "dry_run": true,
                "hardware_applied": false,
                "targets": plans.iter().map(|(index, name, target)| serde_json::json!({
                    "fan_index": index,
                    "fan_name": name,
                    "requested_percent": target,
                    "accepted_target_percent": null,
                    "reported_percent": null,
                })).collect::<Vec<_>>(),
                "message": "valid preset request; no write performed (dry-run)"
            }));
        }
        let profile_was_active = self.state.lock().expect("poisoned mutex").profile.active;
        self.end_active_profile("fixed preset takeover")?;
        for (index, name, target) in &plans {
            let output = match self.backend.set_speed(*index, *target) {
                Ok(output) => output,
                Err(error) => {
                    let recovery = self.fail_safe_after_control_failure(&format!(
                        "fan {index} command failed: {error}"
                    ));
                    return Err(ApiError::new(
                        "backend_unavailable",
                        format!("{error}; {}", recovery.sentence()),
                    ));
                }
            };
            if !output.success {
                let error = output.combined_error();
                let recovery = self.fail_safe_after_control_failure(&format!(
                    "fan {index} command failed: {error}"
                ));
                return Err(ApiError::new(
                    "backend_failed",
                    format!("{error}; {}", recovery.sentence()),
                ));
            }
            let _ = name;
        }
        self.refresh();
        let post = self.state.lock().expect("poisoned mutex").nbfc.clone();
        let Some(post_status) = post.as_ref() else {
            let recovery =
                self.fail_safe_after_control_failure("NBFC status unavailable after preset writes");
            return Err(ApiError::new(
                "verification_failed",
                format!(
                    "NBFC status unavailable after preset; {}",
                    recovery.sentence()
                ),
            ));
        };
        let mut targets = Vec::new();
        let mut leases = Vec::new();
        for (index, name, requested) in &plans {
            let fan = post_status.fan(*index);
            let accepted = fan.and_then(|fan| fan.target_speed);
            let reported = fan.and_then(|fan| fan.current_speed);
            let mode_ok = fan.and_then(|fan| fan.auto_mode) == Some(false);
            let target_ok = accepted
                .map(|value| (value - requested).abs() <= 1.0)
                .unwrap_or(false);
            targets.push(serde_json::json!({
                "fan_index": index,
                "fan_name": name,
                "requested_percent": requested,
                "accepted_target_percent": accepted,
                "reported_percent": reported,
            }));
            if !mode_ok || !target_ok {
                let recovery = self.fail_safe_after_control_failure(
                    "preset target or manual mode was not confirmed",
                );
                return Err(ApiError::new(
                    "verification_failed",
                    format!("preset target not confirmed; {}", recovery.sentence()),
                ));
            }
            leases.push(ManualLease {
                fan_index: *index,
                percent: *requested,
                expires_at_ms: now_ms() + safety_config.manual_ttl_ms,
            });
        }
        let safety = SafetyReport {
            state: SafetyState::Manual,
            reasons: Vec::new(),
            manual: None,
            preset: Some(leases),
            updated_at_ms: now_ms(),
        };
        self.set_safety(safety);
        Ok(serde_json::json!({
            "schema": "neosense.preset_result/v1",
            "preset": preset.name,
            "verified": true,
            "dry_run": false,
            "hardware_applied": true,
            "targets": targets,
            "message": if profile_was_active {
                "active profile ended; preset targets confirmed by backend; EC readback is not RPM"
            } else {
                "preset targets confirmed by backend; EC readback is not RPM"
            }
        }))
    }

    fn fail_safe_after_control_failure(&self, reason: &str) -> RecoveryOutcome {
        match self.auto_internal("fail-safe recovery") {
            Ok(_) => {
                self.set_safety(SafetyReport {
                    state: SafetyState::FailSafe,
                    reasons: vec![
                        reason.to_string(),
                        "automatic mode confirmed by backend".to_string(),
                    ],
                    manual: None,
                    preset: None,
                    updated_at_ms: now_ms(),
                });
                RecoveryOutcome {
                    verified: true,
                    error: None,
                }
            }
            Err(error) => {
                self.set_safety(SafetyReport {
                    state: SafetyState::FailSafe,
                    reasons: vec![
                        reason.to_string(),
                        format!("automatic recovery failed: {}", error.message),
                    ],
                    manual: None,
                    preset: None,
                    updated_at_ms: now_ms(),
                });
                RecoveryOutcome {
                    verified: false,
                    error: Some(error.message),
                }
            }
        }
    }

    fn mark_profile_inactive(&self, reason: &str) {
        let mut state = self.state.lock().expect("poisoned mutex");
        if state.profile.active || state.profile.selected.is_some() {
            state.profile.active = false;
            state.profile.mode = None;
            state.profile.applied_fans.clear();
            state.profile.reason = Some(reason.to_string());
        }
    }

    fn manual_safety_from_status(
        &self,
        status: &NbfcStatus,
        reason: &str,
    ) -> Result<SafetyReport, ApiError> {
        if status.all_auto() {
            return Ok(SafetyReport::safe_auto());
        }
        let mut leases = Vec::new();
        for remaining in &status.fans {
            match remaining.auto_mode {
                Some(false) => {
                    let Some(percent) = remaining.target_speed else {
                        return Err(ApiError::new(
                            "verification_failed",
                            format!(
                                "fan {} has no reliable manual target ({reason})",
                                remaining.index
                            ),
                        ));
                    };
                    leases.push(ManualLease {
                        fan_index: remaining.index,
                        percent,
                        expires_at_ms: now_ms() + self.config.safety.manual_ttl_ms,
                    });
                }
                Some(true) => {}
                None => {
                    return Err(ApiError::new(
                        "verification_failed",
                        format!("fan {} mode is unknown ({reason})", remaining.index),
                    ));
                }
            }
        }
        if leases.is_empty() {
            return Err(ApiError::new(
                "verification_failed",
                format!("remaining manual fan state is inconsistent ({reason})"),
            ));
        }
        Ok(SafetyReport {
            state: SafetyState::Manual,
            reasons: vec![reason.to_string()],
            manual: None,
            preset: Some(leases),
            updated_at_ms: now_ms(),
        })
    }

    fn auto_fan_internal(&self, fan: u32, reason: &str) -> Result<ControlResult, ApiError> {
        let applied_before = {
            let state = self.state.lock().expect("poisoned mutex");
            if state.profile.active {
                Some(state.profile.applied_fans.clone())
            } else {
                None
            }
        };
        let output = match self.backend.set_auto_fan(fan) {
            Ok(output) => output,
            Err(error) => {
                let recovery = self.fail_safe_after_control_failure(&format!(
                    "fan {fan} automatic command failed: {error}"
                ));
                return Err(ApiError::new(
                    "backend_unavailable",
                    format!("{error}; {}", recovery.sentence()),
                ));
            }
        };
        if !output.success {
            let error = output.combined_error();
            let recovery = self.fail_safe_after_control_failure(&format!(
                "fan {fan} automatic command failed: {error}"
            ));
            return Err(ApiError::new(
                "backend_failed",
                format!("{error}; {}", recovery.sentence()),
            ));
        }
        self.refresh();
        let status = self.state.lock().expect("poisoned mutex").nbfc.clone();
        let Some(status) = status else {
            let recovery = self.fail_safe_after_control_failure(&format!(
                "NBFC status unavailable after fan {fan} automatic command"
            ));
            return Err(ApiError::new(
                "verification_failed",
                format!("NBFC status unavailable; {}", recovery.sentence()),
            ));
        };
        let Some(target_fan) = status.fan(fan) else {
            let recovery = self.fail_safe_after_control_failure(&format!(
                "fan {fan} disappeared after automatic command"
            ));
            return Err(ApiError::new(
                "verification_failed",
                format!("fan disappeared; {}", recovery.sentence()),
            ));
        };
        if target_fan.auto_mode != Some(true) {
            let recovery = self.fail_safe_after_control_failure(&format!(
                "fan {fan} automatic mode was not confirmed"
            ));
            return Err(ApiError::new(
                "verification_failed",
                format!("automatic mode not confirmed; {}", recovery.sentence()),
            ));
        }

        if let Some(applied) = applied_before {
            let remaining: Vec<u32> = applied.into_iter().filter(|index| *index != fan).collect();
            for index in &remaining {
                if status.fan(*index).and_then(|item| item.auto_mode) != Some(false) {
                    let recovery = self.fail_safe_after_control_failure(&format!(
                        "profile fan {index} lost manual ownership after fan {fan} automatic request"
                    ));
                    return Err(ApiError::new(
                        "verification_failed",
                        format!(
                            "profile fan {index} is no longer under profile control; {}",
                            recovery.sentence()
                        ),
                    ));
                }
            }
            if remaining.is_empty() {
                let safety = match self.manual_safety_from_status(
                    &status,
                    "profile ended after per-fan automatic request",
                ) {
                    Ok(safety) => safety,
                    Err(error) => {
                        let recovery = self.fail_safe_after_control_failure(&error.message);
                        return Err(ApiError::new(
                            error.code,
                            format!("{}; {}", error.message, recovery.sentence()),
                        ));
                    }
                };
                self.state.lock().expect("poisoned mutex").profile = ProfileState::default();
                self.set_safety(safety.clone());
                return Ok(ControlResult {
                    action: "auto".to_string(),
                    fan_index: Some(fan),
                    requested_percent: None,
                    target_percent: None,
                    reported_percent: None,
                    readback_plausible: None,
                    verified: true,
                    dry_run: false,
                    message: format!(
                        "fan {fan} automatic mode confirmed; profile ended because no profile fans remain ({reason})"
                    ),
                    safety,
                });
            }
            {
                let mut state = self.state.lock().expect("poisoned mutex");
                state.profile.applied_fans = remaining.clone();
                state.profile.mode = Some(RegulationMode::Active);
                state.profile.reason = Some(format!(
                    "fan {fan} is automatic; profile continues for applied fans {remaining:?}"
                ));
            }
            let safety = SafetyReport {
                state: SafetyState::Profile,
                reasons: vec![format!(
                    "fan {fan} is automatic; profile continues for applied fans {remaining:?} ({reason})"
                )],
                manual: None,
                preset: None,
                updated_at_ms: now_ms(),
            };
            self.set_safety(safety.clone());
            return Ok(ControlResult {
                action: "auto".to_string(),
                fan_index: Some(fan),
                requested_percent: None,
                target_percent: None,
                reported_percent: None,
                readback_plausible: None,
                verified: true,
                dry_run: false,
                message: format!(
                    "fan {fan} automatic mode confirmed; profile continues for fans {remaining:?}"
                ),
                safety,
            });
        }

        let safety = match self.manual_safety_from_status(
            &status,
            &format!("fan {fan} automatic mode confirmed ({reason})"),
        ) {
            Ok(safety) => safety,
            Err(error) => {
                let recovery = self.fail_safe_after_control_failure(&error.message);
                return Err(ApiError::new(
                    error.code,
                    format!("{}; {}", error.message, recovery.sentence()),
                ));
            }
        };
        if safety.state == SafetyState::SafeAuto {
            self.state.lock().expect("poisoned mutex").profile = ProfileState::default();
        }
        self.set_safety(safety.clone());
        Ok(ControlResult {
            action: "auto".to_string(),
            fan_index: Some(fan),
            requested_percent: None,
            target_percent: None,
            reported_percent: None,
            readback_plausible: None,
            verified: true,
            dry_run: false,
            message: if safety.state == SafetyState::SafeAuto {
                format!("fan {fan} automatic mode confirmed; all fans automatic")
            } else {
                format!("fan {fan} automatic mode confirmed; other manual fans unchanged")
            },
            safety,
        })
    }

    fn manual_enter_internal(&self, request: ManualEnter) -> Result<serde_json::Value, ApiError> {
        let (status, _, _, _) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.nbfc.clone(),
                state.sensors.clone(),
                state.sensor_config.clone(),
                self.config.safety.clone(),
            )
        };
        let Some(status) = status else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC status unavailable; manual mode refused",
            ));
        };
        let fans: Vec<&crate::nbfc::NbfcFan> =
            match request.fan {
                Some(index) => vec![status.fan(index).ok_or_else(|| {
                    ApiError::new("bad_request", format!("fan {index} not found"))
                })?],
                None => status.fans.iter().collect(),
            };
        if fans.is_empty() {
            return Err(ApiError::new(
                "backend_unavailable",
                "no NBFC fan available",
            ));
        }
        let mut targets = Vec::with_capacity(fans.len());
        for fan in fans {
            // NBFC can report RequestedSpeed=0.0 as a placeholder in automatic
            // mode. TargetSpeed is the current automatic/manual target and is
            // the safe value to preserve when entering manual mode.
            let target = fan.target_speed.or(fan.requested_speed).ok_or_else(|| {
                ApiError::new(
                    "unsafe_refused",
                    format!("fan {} has no reliable current target", fan.index),
                )
            })?;
            targets.push((fan.index, target));
        }
        self.set_many_internal("manual", targets, request.confirm, request.dry_run)
    }

    fn set_many_internal(
        &self,
        action: &str,
        targets: Vec<(u32, f64)>,
        confirm: bool,
        dry_run: bool,
    ) -> Result<serde_json::Value, ApiError> {
        if !confirm {
            return Err(ApiError::new(
                "confirmation_required",
                "manual control requires explicit confirmation",
            ));
        }
        if targets.is_empty() {
            return Err(ApiError::new("bad_request", "no fan targets supplied"));
        }
        if targets.len() > 16 {
            return Err(ApiError::new("bad_request", "too many fan targets"));
        }
        let mut seen = HashSet::new();
        for (fan, percent) in &targets {
            if !percent.is_finite() || !(0.0..=100.0).contains(percent) {
                return Err(ApiError::new(
                    "bad_request",
                    format!("fan {fan}: percentage must be between 0 and 100"),
                ));
            }
            if !seen.insert(*fan) {
                return Err(ApiError::new(
                    "bad_request",
                    format!("fan {fan} is specified more than once"),
                ));
            }
        }
        let capabilities = self.live_capabilities();
        let (snapshot, status, sensor_config, safety_config) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.sensors.clone(),
                state.nbfc.clone(),
                state.sensor_config.clone(),
                self.config.safety.clone(),
            )
        };
        let Some(status) = status else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC status unavailable; manual control refused",
            ));
        };
        let Some(capabilities) = capabilities else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC capabilities unavailable; manual control refused",
            ));
        };
        struct Plan {
            index: u32,
            name: String,
            role: FanRole,
            percent: f64,
        }
        let mut plans = Vec::with_capacity(targets.len());
        for (index, percent) in targets {
            let Some(fan) = status.fan(index) else {
                return Err(ApiError::new(
                    "bad_request",
                    format!("fan {index} not found"),
                ));
            };
            let decision = assess_manual_request(
                &snapshot,
                &status,
                &capabilities,
                sensor_config.as_ref(),
                index,
                percent,
                &safety_config,
                now_ms(),
            );
            if !decision.allowed {
                return Err(ApiError::new(
                    "unsafe_refused",
                    "manual control refused by the safety policy before any write",
                )
                .with_details(decision.reasons));
            }
            plans.push(Plan {
                index,
                name: fan.name.clone(),
                role: fan_role(&fan.name),
                percent,
            });
        }
        let requested = |plan: &Plan| {
            serde_json::json!({
                "fan_index": plan.index,
                "fan_name": plan.name,
                "role": plan.role,
                "requested_percent": plan.percent,
            })
        };
        if dry_run {
            return Ok(serde_json::json!({
                "schema": "neosense.control_result_many/v1",
                "action": action,
                "dry_run": true,
                "verified": false,
                "partial": false,
                "automatic_recovery": false,
                "results": plans.iter().map(|plan| {
                    let mut value = requested(plan);
                    value["command_accepted"] = serde_json::Value::Null;
                    value["target_percent"] = serde_json::Value::Null;
                    value["reported_percent"] = serde_json::Value::Null;
                    value["readback_plausible"] = serde_json::Value::Null;
                    value["mode"] = serde_json::Value::Null;
                    value["verified"] = serde_json::Value::Bool(false);
                    value["error"] = serde_json::Value::Null;
                    value
                }).collect::<Vec<_>>(),
                "message": "valid request; no write performed (dry-run)",
            }));
        }
        let profile_was_active = self.state.lock().expect("poisoned mutex").profile.active;
        self.end_active_profile("manual control takeover")?;
        let mut commands = Vec::with_capacity(plans.len());
        for plan in &plans {
            let result = match self.backend.set_speed(plan.index, plan.percent) {
                Ok(output) => (
                    output.success,
                    if output.success {
                        None
                    } else {
                        Some(output.combined_error())
                    },
                ),
                Err(error) => (false, Some(error)),
            };
            commands.push(result);
        }
        self.refresh();
        let post = self.state.lock().expect("poisoned mutex").nbfc.clone();
        let Some(post) = post else {
            let recovery = self.fail_safe_after_control_failure(&format!(
                "{action}: NBFC status unavailable after writes"
            ));
            return Ok(serde_json::json!({
                "schema": "neosense.control_result_many/v1",
                "action": action,
                "dry_run": false,
                "verified": false,
                "partial": false,
                "automatic_recovery": recovery.verified,
                "recovery_failed": !recovery.verified,
                "recovery_error": recovery.error,
                "results": plans.iter().map(|plan| {
                    let mut value = requested(plan);
                    value["command_accepted"] = serde_json::Value::Null;
                    value["target_percent"] = serde_json::Value::Null;
                    value["reported_percent"] = serde_json::Value::Null;
                    value["readback_plausible"] = serde_json::Value::Null;
                    value["mode"] = serde_json::Value::Null;
                    value["verified"] = serde_json::Value::Bool(false);
                    value["error"] = serde_json::Value::String("NBFC status unavailable after write".into());
                    value
                }).collect::<Vec<_>>(),
                "message": format!(
                    "NBFC status unavailable after writes; {}",
                    recovery.sentence()
                ),
            }));
        };
        let mut results = Vec::with_capacity(plans.len());
        let mut verified_count = 0usize;
        for (index, plan) in plans.iter().enumerate() {
            let fan = post.fan(plan.index);
            let target = fan.and_then(|fan| fan.target_speed);
            let reported = fan.and_then(|fan| fan.current_speed);
            let mode = fan.and_then(|fan| fan.auto_mode);
            let target_ok = target
                .map(|value| (value - plan.percent).abs() <= 1.0)
                .unwrap_or(false);
            let mode_ok = mode == Some(false);
            let readback_plausible = match (reported, target) {
                (Some(value), Some(target)) => {
                    (0.0..=100.0).contains(&value) && (value - target).abs() <= 20.0
                }
                (Some(value), None) => (0.0..=100.0).contains(&value),
                _ => false,
            };
            let (command_accepted, command_error) = &commands[index];
            let verified = *command_accepted && target_ok && mode_ok;
            if verified {
                verified_count += 1;
            }
            let mut value = requested(plan);
            value["command_accepted"] = serde_json::Value::Bool(*command_accepted);
            value["target_percent"] = serde_json::json!(target);
            value["reported_percent"] = serde_json::json!(reported);
            value["readback_plausible"] = serde_json::Value::Bool(readback_plausible);
            value["mode"] = serde_json::json!(match mode {
                Some(true) => "auto",
                Some(false) => "manual",
                None => "unknown",
            });
            value["verified"] = serde_json::Value::Bool(verified);
            value["error"] = command_error
                .as_ref()
                .map(|error| serde_json::Value::String(error.clone()))
                .unwrap_or(serde_json::Value::Null);
            results.push(value);
        }
        if verified_count == plans.len() {
            let safety = match self.manual_safety_from_status(
                &post,
                "manual targets confirmed; all manual fans are leased",
            ) {
                Ok(safety) => safety,
                Err(error) => {
                    let recovery = self.fail_safe_after_control_failure(&error.message);
                    return Ok(serde_json::json!({
                        "schema": "neosense.control_result_many/v1",
                        "action": action,
                        "dry_run": false,
                        "verified": false,
                        "partial": true,
                        "automatic_recovery": recovery.verified,
                        "recovery_failed": !recovery.verified,
                        "recovery_error": recovery.error,
                        "results": results,
                        "message": format!("{}; {}", error.message, recovery.sentence()),
                    }));
                }
            };
            self.set_safety(safety.clone());
            return Ok(serde_json::json!({
                "schema": "neosense.control_result_many/v1",
                "action": action,
                "dry_run": false,
                "verified": true,
                "partial": false,
                "automatic_recovery": false,
                "results": results,
                "safety": safety,
                "message": if profile_was_active {
                    format!(
                        "{verified_count}/{} fan(s) verified; active profile ended; manual targets and modes confirmed by backend",
                        plans.len()
                    )
                } else {
                    format!("{verified_count}/{} fan(s) verified; manual targets and modes confirmed by backend", plans.len())
                },
            }));
        }
        let recovery = self.fail_safe_after_control_failure(&format!(
            "{action}: only {verified_count}/{} fan(s) verified",
            plans.len()
        ));
        Ok(serde_json::json!({
            "schema": "neosense.control_result_many/v1",
            "action": action,
            "dry_run": false,
            "verified": false,
            "partial": verified_count > 0,
            "automatic_recovery": recovery.verified,
            "recovery_failed": !recovery.verified,
            "recovery_error": recovery.error,
            "results": results,
            "message": format!(
                "{}; {}",
                if verified_count > 0 {
                    format!("partial success: {verified_count}/{} fan(s) verified", plans.len())
                } else {
                    format!("no fan verified (0/{})", plans.len())
                },
                recovery.sentence()
            ),
        }))
    }

    fn auto_internal(&self, reason: &str) -> Result<ControlResult, ApiError> {
        let profile_was_active = self.state.lock().expect("poisoned mutex").profile.active;
        let output = match self.backend.set_auto() {
            Ok(output) => output,
            Err(error) => {
                self.mark_profile_inactive(&format!(
                    "automatic recovery could not be sent during {reason}: {error}"
                ));
                self.set_safety(SafetyReport {
                    state: SafetyState::FailSafe,
                    reasons: vec![format!(
                        "automatic recovery could not be sent during {reason}: {error}"
                    )],
                    manual: None,
                    preset: None,
                    updated_at_ms: now_ms(),
                });
                return Err(ApiError::new("backend_unavailable", error));
            }
        };
        if !output.success {
            let error = output.combined_error();
            self.mark_profile_inactive(&format!(
                "automatic recovery failed during {reason}: {error}"
            ));
            self.set_safety(SafetyReport {
                state: SafetyState::FailSafe,
                reasons: vec![format!(
                    "automatic recovery failed during {reason}: {error}"
                )],
                manual: None,
                preset: None,
                updated_at_ms: now_ms(),
            });
            return Err(ApiError::new("backend_failed", error));
        }
        self.refresh();
        let status = self.state.lock().expect("poisoned mutex").nbfc.clone();
        let verified = status.as_ref().map(NbfcStatus::all_auto).unwrap_or(false);
        if !verified {
            self.mark_profile_inactive(&format!(
                "automatic mode was not confirmed during {reason}"
            ));
            let safety = SafetyReport {
                state: SafetyState::FailSafe,
                reasons: vec![format!("automatic mode not confirmed after: {reason}")],
                manual: None,
                preset: None,
                updated_at_ms: now_ms(),
            };
            self.set_safety(safety);
            return Err(ApiError::new(
                "verification_failed",
                "automatic command accepted but mode not confirmed",
            ));
        }

        let safety = SafetyReport::safe_auto();
        self.set_safety(safety.clone());
        self.state.lock().expect("poisoned mutex").profile = ProfileState::default();
        Ok(ControlResult {
            action: "auto".to_string(),
            fan_index: None,
            requested_percent: None,
            target_percent: None,
            reported_percent: None,
            readback_plausible: None,
            verified,
            dry_run: false,
            message: if profile_was_active {
                "active profile ended; NBFC automatic mode confirmed".to_string()
            } else {
                "NBFC automatic mode confirmed".to_string()
            },
            safety,
        })
    }

    fn manual_internal(&self, request: ManualSet) -> Result<ControlResult, ApiError> {
        if !request.confirm {
            return Err(ApiError::new(
                "confirmation_required",
                "manual control requires explicit confirmation",
            ));
        }
        let capabilities = self.live_capabilities();
        let (snapshot, status, sensor_config, safety_config) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.sensors.clone(),
                state.nbfc.clone(),
                state.sensor_config.clone(),
                self.config.safety.clone(),
            )
        };
        let Some(status) = status else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC status unavailable; manual control refused",
            ));
        };
        let Some(capabilities) = capabilities else {
            return Err(ApiError::new(
                "backend_unavailable",
                "NBFC capabilities unavailable; manual control refused",
            ));
        };
        let decision: PolicyDecision = assess_manual_request(
            &snapshot,
            &status,
            &capabilities,
            sensor_config.as_ref(),
            request.fan,
            request.percent,
            &safety_config,
            now_ms(),
        );
        if !decision.allowed {
            return Err(ApiError::new(
                "unsafe_refused",
                "manual control refused by the safety policy",
            )
            .with_details(decision.reasons));
        }
        if request.dry_run {
            return Ok(ControlResult {
                action: "set".to_string(),
                fan_index: Some(request.fan),
                requested_percent: Some(request.percent),
                target_percent: None,
                reported_percent: None,
                readback_plausible: None,
                verified: false,
                dry_run: true,
                message: "valid request; no write performed (dry-run)".to_string(),
                safety: self.status().safety,
            });
        }

        let profile_was_active = self.state.lock().expect("poisoned mutex").profile.active;
        self.end_active_profile("manual control takeover")?;
        let output = self
            .backend
            .set_speed(request.fan, request.percent)
            .map_err(|e| ApiError::new("backend_unavailable", e))?;
        if !output.success {
            return Err(ApiError::new("backend_failed", output.combined_error()));
        }

        self.refresh();
        let (updated, post_snapshot, post_sensor_config) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.nbfc.clone(),
                state.sensors.clone(),
                state.sensor_config.clone(),
            )
        };
        let Some(post_status) = updated.as_ref() else {
            let recovery =
                self.fail_safe_after_control_failure("NBFC status unavailable after manual write");
            return Err(ApiError::new(
                "verification_failed",
                format!(
                    "NBFC status unavailable after write; {}",
                    recovery.sentence()
                ),
            ));
        };
        let post_decision = assess_manual_request(
            &post_snapshot,
            post_status,
            &capabilities,
            post_sensor_config.as_ref(),
            request.fan,
            request.percent,
            &safety_config,
            now_ms(),
        );
        if !post_decision.allowed {
            let recovery = self.fail_safe_after_control_failure(&format!(
                "sensor or source became invalid after manual write: {}",
                post_decision.reasons.join("; ")
            ));
            return Err(ApiError::new(
                "verification_failed",
                format!(
                    "sensor or source became invalid after write; {}",
                    recovery.sentence()
                ),
            ));
        }
        let fan = post_status.fan(request.fan);
        let target = fan.and_then(|f| f.target_speed);
        let target_ok = target
            .map(|target| (target - request.percent).abs() <= 1.0)
            .unwrap_or(false);
        let mode_ok = fan.and_then(|f| f.auto_mode) == Some(false);
        let reported = fan.and_then(|f| f.current_speed);
        // A range check alone is not enough: the audit machine reports a
        // clamped 100% against a 35% target. Only call the readback plausible
        // when it is in range and broadly consistent with the target.
        let readback_plausible = match (reported, target) {
            (Some(value), Some(target)) => {
                (0.0..=100.0).contains(&value) && (value - target).abs() <= 20.0
            }
            (Some(value), None) => (0.0..=100.0).contains(&value),
            _ => false,
        };

        if !target_ok || !mode_ok {
            let recovery = self.fail_safe_after_control_failure(
                "manual target or manual mode was not confirmed after write",
            );
            return Err(ApiError::new(
                "verification_failed",
                format!("speed not confirmed; {}", recovery.sentence()),
            ));
        }

        let safety = match self.manual_safety_from_status(
            post_status,
            "manual target confirmed; all manual fans are leased",
        ) {
            Ok(safety) => safety,
            Err(error) => {
                let recovery = self.fail_safe_after_control_failure(&error.message);
                return Err(ApiError::new(
                    error.code,
                    format!("{}; {}", error.message, recovery.sentence()),
                ));
            }
        };
        self.set_safety(safety.clone());
        Ok(ControlResult {
            action: "set".to_string(),
            fan_index: Some(request.fan),
            requested_percent: Some(request.percent),
            target_percent: fan.and_then(|f| f.target_speed),
            reported_percent: reported,
            readback_plausible: Some(readback_plausible),
            verified: target_ok && mode_ok,
            dry_run: false,
            message: if profile_was_active {
                "active profile ended; target and manual mode confirmed by NBFC".to_string()
            } else {
                "target and manual mode confirmed by NBFC".to_string()
            },
            safety,
        })
    }

    fn set_safety(&self, safety: SafetyReport) {
        self.state.lock().expect("poisoned mutex").safety = safety;
    }

    /// Periodic safety enforcement. Called by the sampler thread.
    pub fn tick(&self) {
        let _control = self.lock_control();
        let (profile_active, profile_block) = {
            let state = self.state.lock().expect("poisoned mutex");
            let active = state.safety.state == SafetyState::Profile && state.profile.active;
            let block = if active {
                if now_ms().saturating_sub(state.sensors.collected_at_ms)
                    > self.config.safety.stale_after_ms
                {
                    Some("sensor snapshot became stale".to_string())
                } else if state
                    .capabilities
                    .as_ref()
                    .is_some_and(|capabilities| !capabilities.access_controlled)
                {
                    Some("NBFC socket access control was lost".to_string())
                } else if state
                    .capabilities
                    .as_ref()
                    .is_some_and(|capabilities| !capabilities.protected_manual)
                {
                    Some("NBFC no longer provides protected manual control".to_string())
                } else if state.nbfc.as_ref().and_then(|status| status.read_only) == Some(true) {
                    Some("NBFC entered read-only mode".to_string())
                } else if state.nbfc.as_ref().is_some_and(NbfcStatus::any_critical) {
                    Some("NBFC reported critical mode".to_string())
                } else {
                    None
                }
            } else {
                None
            };
            (active, block)
        };
        if let Some(reason) = profile_block {
            let recovery = self
                .fail_safe_after_control_failure(&format!("profile regulation stopped: {reason}"));
            let mut state = self.state.lock().expect("poisoned mutex");
            state.profile.active = false;
            state.profile.mode = None;
            state.profile.applied_fans.clear();
            state.profile.reason = Some(format!(
                "profile stopped: {reason}; {}",
                recovery.sentence()
            ));
        } else if profile_active && let Err(error) = self.regulate_profile_once() {
            let recovery = self
                .fail_safe_after_control_failure(&format!("profile regulation stopped: {error}"));
            let mut state = self.state.lock().expect("poisoned mutex");
            state.profile.active = false;
            state.profile.mode = None;
            state.profile.applied_fans.clear();
            state.profile.reason =
                Some(format!("profile stopped: {error}; {}", recovery.sentence()));
        }
        let (leases, critical, cpu_ok, gpu_ok, security_ok) = {
            let state = self.state.lock().expect("poisoned mutex");
            let now = now_ms();
            let security_ok = state
                .capabilities
                .as_ref()
                .map(|capabilities| capabilities.access_controlled)
                .unwrap_or(false);
            let mut leases = Vec::new();
            if let Some(lease) = &state.safety.manual {
                leases.push(lease.clone());
            }
            if let Some(preset) = &state.safety.preset {
                leases.extend(preset.iter().cloned());
            }
            let critical = state
                .nbfc
                .as_ref()
                .map(NbfcStatus::any_critical)
                .unwrap_or(false);
            let cpu_ok = state
                .sensors
                .cpu_package(now, self.config.safety.stale_after_ms)
                .is_some();
            let needs_gpu = leases.iter().any(|lease| {
                state
                    .nbfc
                    .as_ref()
                    .and_then(|s| s.fan(lease.fan_index))
                    .is_some_and(|f| fan_role(&f.name) == FanRole::Gpu)
            });
            let gpu_ok = !needs_gpu
                || state.sensors.gpu_indexes().into_iter().any(|i| {
                    state
                        .sensors
                        .gpu_die(i, now, self.config.safety.stale_after_ms)
                        .is_some()
                });
            (leases, critical, cpu_ok, gpu_ok, security_ok)
        };

        if !leases.is_empty() {
            let expired = leases.iter().any(|lease| now_ms() >= lease.expires_at_ms);
            let security_lost = !security_ok;
            if expired || critical || !cpu_ok || !gpu_ok || security_lost {
                let reason = if expired {
                    "manual lease expired"
                } else if critical {
                    "NBFC critical mode"
                } else if security_lost {
                    "NBFC access control changed while a manual/preset lease was active"
                } else {
                    "critical sensor missing or stale"
                };
                match self.auto_internal(reason) {
                    Ok(_) => {
                        let mut state = self.state.lock().expect("poisoned mutex");
                        if security_lost {
                            state.safety.state = SafetyState::FailSafe;
                            state.safety.reasons = vec![reason.to_string()];
                        } else if !expired && (critical || !cpu_ok || !gpu_ok) {
                            state.safety.state = SafetyState::Degraded;
                            state.safety.reasons = vec![reason.to_string()];
                        }
                    }
                    Err(error) => {
                        let mut state = self.state.lock().expect("poisoned mutex");
                        state.safety.state = SafetyState::FailSafe;
                        state.safety.manual = None;
                        state.safety.preset = None;
                        state.safety.reasons = vec![
                            reason.to_string(),
                            format!("automatic recovery failed: {}", error.message),
                        ];
                        state.safety.updated_at_ms = now_ms();
                    }
                }
            }
        }
    }

    /// Run the supervisor with the real systemd notifier.
    pub fn run(self: &Arc<Self>, shutdown: Arc<AtomicBool>) -> Result<(), String> {
        let notifier: Arc<dyn Notifier> = Arc::new(SystemdNotifier::from_env());
        self.run_with_notifier(shutdown, notifier)
    }

    /// Run the supervisor with an injectable notifier. Tests use a recorder;
    /// production uses `SystemdNotifier`.
    pub fn run_with_notifier(
        self: &Arc<Self>,
        shutdown: Arc<AtomicBool>,
        notifier: Arc<dyn Notifier>,
    ) -> Result<(), String> {
        let socket = &self.config.socket;
        if let Some(parent) = socket.parent() {
            prepare_runtime_dir(parent)?;
        }
        if socket.exists() {
            if UnixStream::connect(socket).is_ok() {
                return Err(format!(
                    "a supervisor is already listening on {}",
                    socket.display()
                ));
            }
            std::fs::remove_file(socket).map_err(|e| format!("remove stale socket: {e}"))?;
        }

        // The runtime directory is 0750, so non-members cannot traverse it.
        // The umask avoids a brief world-readable socket creation window.
        let old_umask = unsafe { libc::umask(0o007) };
        let bind_result = UnixListener::bind(socket);
        unsafe {
            libc::umask(old_umask);
        }
        let listener = bind_result.map_err(|e| format!("bind {}: {e}", socket.display()))?;
        if let Err(error) = apply_socket_security(socket) {
            let _ = std::fs::remove_file(socket);
            return Err(error);
        }
        if let Err(error) = listener.set_nonblocking(true) {
            let _ = std::fs::remove_file(socket);
            return Err(format!("failed to set socket non-blocking: {error}"));
        }

        // Refresh once more after socket setup; READY is only sent when the
        // supervisor has an initial consistent snapshot and the IPC socket is
        // bound, permissioned and accepting.
        self.refresh();
        if let Err(error) = notifier.ready() {
            let _ = std::fs::remove_file(socket);
            return Err(error);
        }

        let sampler = Arc::clone(self);
        let stop = Arc::clone(&shutdown);
        let interval = Duration::from_millis(self.config.interval_ms.max(500));
        let sampler_thread = thread::spawn(move || {
            let step = Duration::from_millis(100);
            while !stop.load(Ordering::Relaxed) {
                sampler.refresh();
                sampler.tick();
                let mut slept = Duration::ZERO;
                while slept < interval && !stop.load(Ordering::Relaxed) {
                    let remaining = interval.saturating_sub(slept);
                    let chunk = remaining.min(step);
                    thread::sleep(chunk);
                    slept += chunk;
                }
            }
        });

        let watchdog_interval = notifier
            .watchdog_interval()
            .map(|duration| (duration / 3).max(Duration::from_secs(1)));
        let mut last_watchdog = Instant::now();
        let mut run_error = None;
        let gate = Arc::new(ConnectionGate::new(self.config.max_connections));

        while !shutdown.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let Some(permit) = gate.try_enter() else {
                        // Bound resource use under a connection flood. The
                        // client observes a closed connection and can retry.
                        drop(stream);
                        continue;
                    };
                    let daemon = Arc::clone(self);
                    thread::spawn(move || {
                        let _permit = permit;
                        handle_connection(daemon, stream);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    run_error = Some(format!("accept: {e}"));
                    break;
                }
            }

            if let Some(period) = watchdog_interval
                && last_watchdog.elapsed() >= period
            {
                last_watchdog = Instant::now();
                let health = self.health();
                if health.healthy {
                    if let Err(error) = notifier.watchdog() {
                        eprintln!("neosensed: failed to send WATCHDOG=1: {error}");
                    }
                } else {
                    eprintln!(
                        "neosensed: watchdog suspended: {}",
                        health.reason.as_deref().unwrap_or("unknown health")
                    );
                }
            }
        }

        if let Err(error) = notifier.stopping() {
            eprintln!("neosensed: failed to send STOPPING=1: {error}");
        }
        let _ = sampler_thread.join();
        let _control = self.lock_control();
        let _ = self.auto_internal("supervisor shutdown");
        let _ = std::fs::remove_file(socket);
        match run_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn evaluate_selected_profile(
    selected: Option<&str>,
    snapshot: &SensorSnapshot,
    nbfc: Option<&NbfcStatus>,
    sensor_config: Option<&NbfcSensorConfig>,
    old_profile: &ProfileState,
    customs: &[CustomProfile],
    custom_error: Option<&str>,
    now: u64,
) -> ProfileState {
    let Some(name) = selected else {
        return ProfileState::default();
    };
    let active = old_profile.active && old_profile.selected.as_deref() == Some(name);
    let mode = if active {
        RegulationMode::Active
    } else {
        RegulationMode::Simulated
    };
    let applied_fans = if active {
        old_profile.applied_fans.clone()
    } else {
        Vec::new()
    };
    let blocked_fans = if active {
        old_profile.blocked_fans.clone()
    } else {
        Vec::new()
    };
    let builtin = crate::profiles::find(name);
    let custom = customs.iter().find(|profile| profile.name == name);
    let (curve, roles) = if let Some(profile) = builtin {
        (profile.curve, None)
    } else if let Some(profile) = custom {
        (Some(profile.curve_spec()), Some(profile.fan_roles.clone()))
    } else if let Some(error) = custom_error {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(mode),
            active,
            applied_fans: applied_fans.clone(),
            blocked_fans: blocked_fans.clone(),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some(format!("custom profile store unavailable: {error}")),
        };
    } else {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(mode),
            active,
            applied_fans: applied_fans.clone(),
            blocked_fans: blocked_fans.clone(),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some(format!("unknown profile: {name}")),
        };
    };
    let Some(curve) = curve else {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(mode),
            active,
            applied_fans: applied_fans.clone(),
            blocked_fans: blocked_fans.clone(),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some(format!("profile {name} has no regulation curve")),
        };
    };
    let Some(status) = nbfc else {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(mode),
            active,
            applied_fans: applied_fans.clone(),
            blocked_fans: blocked_fans.clone(),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some("NBFC status is unavailable".to_string()),
        };
    };
    let same_profile = old_profile.selected.as_deref() == Some(name);
    let fans: Vec<FanObservation> = status
        .fans
        .iter()
        .filter(|fan| {
            roles
                .as_ref()
                .is_none_or(|roles| roles.contains(&fan_role(&fan.name)))
        })
        .map(|fan| {
            let role = fan_role(&fan.name);
            let source =
                sensor_config.and_then(|config| config.fans.iter().find(|f| f.index == fan.index));
            let mapping_ok = temperature_mapping(
                role,
                source,
                snapshot,
                now,
                crate::policy::SafetyConfig::default().stale_after_ms,
            )
            .matched;
            let old_target = if same_profile {
                old_profile
                    .targets
                    .iter()
                    .find(|target| target.fan_index == fan.index)
            } else {
                None
            };
            FanObservation {
                fan_index: fan.index,
                fan_name: fan.name.clone(),
                role,
                mapping_ok,
                previous_target_percent: old_target.and_then(|target| target.target_percent),
                previous_temperature_c: old_target.and_then(|target| target.temperature_c),
            }
        })
        .collect();
    if fans.is_empty() {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(mode),
            active,
            applied_fans: applied_fans.clone(),
            blocked_fans: blocked_fans.clone(),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some("no backend fan matches the profile's selected fan roles".to_string()),
        };
    }
    match evaluate_profile(&curve, snapshot, &fans, mode, now) {
        Ok(targets) => ProfileState {
            selected: Some(name.to_string()),
            mode: Some(mode),
            active,
            applied_fans: applied_fans.clone(),
            blocked_fans: blocked_fans.clone(),
            evaluated_at_ms: now,
            targets,
            reason: Some(if active {
                "active profile; supervisor applies eligible fan targets".to_string()
            } else if custom.is_some() {
                "user curve simulation only; no fan write performed".to_string()
            } else {
                "simulation only; no fan write performed".to_string()
            }),
        },
        Err(error) => ProfileState {
            selected: Some(name.to_string()),
            mode: Some(mode),
            active,
            applied_fans: applied_fans.clone(),
            blocked_fans: blocked_fans.clone(),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some(error),
        },
    }
}

fn health_from_state(state: &State, config: &DaemonConfig) -> DaemonHealth {
    let now = now_ms();
    let max_age = config.interval_ms.saturating_mul(3).max(5_000);
    let age = now.saturating_sub(state.sensors.collected_at_ms);
    if age > max_age {
        return DaemonHealth {
            healthy: false,
            reason: Some(format!("sensor sample too old ({age} ms)")),
        };
    }
    if state.safety.state == SafetyState::Manual
        && state.safety.manual.is_none()
        && state.safety.preset.as_ref().is_none_or(Vec::is_empty)
    {
        return DaemonHealth {
            healthy: false,
            reason: Some("manual state without a consistent lease".to_string()),
        };
    }
    if state.safety.state == SafetyState::Manual && state.nbfc.is_none() {
        return DaemonHealth {
            healthy: false,
            reason: Some("NBFC unavailable during manual control".to_string()),
        };
    }
    if state.safety.state == SafetyState::Profile && (!state.profile.active || state.nbfc.is_none())
    {
        return DaemonHealth {
            healthy: false,
            reason: Some("active profile lost its fan state".to_string()),
        };
    }
    DaemonHealth {
        healthy: true,
        reason: None,
    }
}

fn prepare_runtime_dir(parent: &Path) -> Result<(), String> {
    prepare_runtime_dir_with_group(parent, socket_group().as_deref())
}

fn prepare_runtime_dir_with_group(parent: &Path, group: Option<&str>) -> Result<(), String> {
    // Only apply group ownership/mode when this process created the directory.
    // A custom --socket path may point at a shared parent such as /tmp; changing
    // an existing directory's permissions would be a serious side effect.
    // systemd creates /run/neosense with RuntimeDirectoryMode=0750 and the
    // configured group before ExecStart.
    if !parent.exists() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        if let Some(group) = group {
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o750))
                .map_err(|e| format!("chmod {}: {e}", parent.display()))?;
            chown_path(parent, 0, group_gid(group)?)?;
        }
    }
    Ok(())
}

fn apply_socket_security(socket: &Path) -> Result<(), String> {
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))
        .map_err(|e| format!("chmod socket: {e}"))?;
    if let Some(group) = socket_group() {
        let gid = group_gid(&group)?;
        chown_path(socket, 0, gid)?;
    }
    Ok(())
}

fn socket_group() -> Option<String> {
    std::env::var("NEOSENSE_SOCKET_GROUP")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn group_gid(name: &str) -> Result<u32, String> {
    let name_c = CString::new(name).map_err(|_| "invalid group name".to_string())?;
    let group = unsafe { libc::getgrnam(name_c.as_ptr()) };
    if group.is_null() {
        return Err(format!("system group not found: {name}"));
    }
    let gid = unsafe { (*group).gr_gid };
    Ok(gid as u32)
}

fn chown_path(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    let path_c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("invalid path: {}", path.display()))?;
    let result = unsafe { libc::chown(path_c.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) };
    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "chown {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ))
    }
}

/// Bounds the number of concurrently served IPC connections. The supervisor
/// accepts a connection only when a permit is available; excess clients get a
/// closed connection instead of spawning an unbounded number of threads.
struct ConnectionGate {
    active: AtomicUsize,
    max: usize,
}

impl ConnectionGate {
    fn new(max: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            max: max.max(1),
        }
    }

    fn try_enter(self: &Arc<Self>) -> Option<ConnectionPermit> {
        let mut current = self.active.load(Ordering::Acquire);
        loop {
            if current >= self.max {
                return None;
            }
            match self.active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(ConnectionPermit {
                        gate: Arc::clone(self),
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    #[cfg(test)]
    fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

struct ConnectionPermit {
    gate: Arc<ConnectionGate>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.gate.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn handle_connection(daemon: Arc<Daemon>, stream: UnixStream) {
    let mut stream = stream;
    // A local group member could otherwise hold an accepted connection open
    // forever and pin one server thread. Bound both directions.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(&stream);
    let request: Request = match read_message(&mut reader) {
        Ok(request) => request,
        Err(_) => return,
    };
    let response = daemon.handle_request(&request);
    let _ = write_message(&mut stream, &response);
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("socket", &self.config.socket)
            .finish_non_exhaustive()
    }
}

/// Decide whether the cached NBFC client version must be re-probed.
///
/// The version is cached to avoid spawning `nbfc --version` on every sampling
/// interval, but it is not immutable for the lifetime of the supervisor: an
/// NBFC upgrade replaces the client and restarts the service. A changed
/// `nbfc_service` MainPID, an expired cache or an unknown version all force a
/// re-probe so capability gates cannot stay pinned to an old backend.
fn version_needs_refresh(
    now_ms: u64,
    cached_version: Option<NbfcVersion>,
    cached_pid: Option<u32>,
    probed_at_ms: u64,
    service_pid: Option<u32>,
    ttl_ms: u64,
) -> bool {
    cached_version.is_none()
        || cached_pid != service_pid
        || now_ms.saturating_sub(probed_at_ms) >= ttl_ms
}

/// Fail-safe helper for systemd `ExecStopPost`. It sets NBFC auto and verifies
/// the result, without requiring the daemon to be alive.
pub fn fail_safe(backend: &dyn FanBackend) -> Result<String, String> {
    let output = backend.set_auto()?;
    if !output.success {
        return Err(output.combined_error());
    }
    let version = backend.version()?;
    let status = backend.status(version)?;
    if status.all_auto() {
        Ok("NBFC automatic mode confirmed".to_string())
    } else {
        Err("NBFC automatic mode not confirmed".to_string())
    }
}

pub fn socket_path_from_env() -> PathBuf {
    std::env::var_os("NEOSENSE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/run/neosense/neosensed.sock").to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SensorKind, SensorProvider, SensorRole, SensorSample, Unit};
    use crate::nbfc::{NbfcFan, NbfcFanSensor};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;

    struct FakeBackend {
        manual_targets: Mutex<HashMap<u32, f64>>,
        version: Mutex<NbfcVersion>,
        fail_auto: bool,
        fail_auto_from_call: Mutex<Option<usize>>,
        critical: Mutex<bool>,
        hide_fan: Mutex<Option<u32>>,
        version_calls: AtomicUsize,
        config_calls: AtomicUsize,
        set_speed_calls: AtomicUsize,
        auto_calls: AtomicUsize,
        auto_fan_calls: AtomicUsize,
        fail_speed_for: Mutex<Option<u32>>,
        ignore_speed: bool,
        two_cpu_fans: bool,
        gpu_second: bool,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                manual_targets: Mutex::new(HashMap::new()),
                version: Mutex::new(NbfcVersion {
                    major: 0,
                    minor: 5,
                    patch: 4,
                }),
                fail_auto: false,
                fail_auto_from_call: Mutex::new(None),
                critical: Mutex::new(false),
                hide_fan: Mutex::new(None),
                version_calls: AtomicUsize::new(0),
                config_calls: AtomicUsize::new(0),
                set_speed_calls: AtomicUsize::new(0),
                auto_calls: AtomicUsize::new(0),
                auto_fan_calls: AtomicUsize::new(0),

                fail_speed_for: Mutex::new(None),
                ignore_speed: false,
                two_cpu_fans: false,
                gpu_second: false,
            }
        }

        fn with_auto_failure() -> Self {
            let mut backend = Self::new();
            backend.fail_auto = true;
            backend
        }

        fn with_two_cpu_fans() -> Self {
            let mut backend = Self::new();
            backend.two_cpu_fans = true;
            backend
        }

        fn with_cpu_and_gpu_fans() -> Self {
            let mut backend = Self::with_two_cpu_fans();
            backend.gpu_second = true;
            backend
        }

        fn with_failing_second_fan() -> Self {
            let backend = Self::with_two_cpu_fans();
            *backend.fail_speed_for.lock().unwrap() = Some(1);
            backend
        }

        fn with_ignored_target() -> Self {
            let mut backend = Self::new();
            backend.ignore_speed = true;
            backend
        }

        fn with_version(major: u32, minor: u32, patch: u32) -> Self {
            let backend = Self::new();
            backend.set_version(NbfcVersion {
                major,
                minor,
                patch,
            });
            backend
        }

        fn set_version(&self, version: NbfcVersion) {
            *self.version.lock().unwrap() = version;
        }
    }

    impl FanBackend for FakeBackend {
        fn version(&self) -> Result<NbfcVersion, String> {
            self.version_calls.fetch_add(1, Ordering::Relaxed);
            Ok(*self.version.lock().unwrap())
        }
        fn status(&self, _version: NbfcVersion) -> Result<NbfcStatus, String> {
            let manual = self.manual_targets.lock().unwrap().clone();
            let fan_auto = |index: u32| !manual.contains_key(&index);
            let target = |index: u32| manual.get(&index).copied().unwrap_or(35.0);
            let mut fans = vec![NbfcFan {
                index: 0,
                name: "CPU fan".into(),
                temperature: Some(55.0),
                critical_temperature: Some(95.0),
                auto_mode: Some(fan_auto(0)),
                critical: Some(*self.critical.lock().unwrap()),
                current_speed: Some(target(0)),
                target_speed: Some(target(0)),
                // Live NBFC can report 0.0 as an automatic-mode placeholder.
                requested_speed: Some(if fan_auto(0) { 0.0 } else { target(0) }),
                speed_steps: Some(100),
            }];
            if self.two_cpu_fans {
                fans.push(NbfcFan {
                    index: 1,
                    name: if self.gpu_second {
                        "GPU fan".into()
                    } else {
                        "CPU fan 2".into()
                    },
                    temperature: Some(55.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(fan_auto(1)),
                    critical: Some(*self.critical.lock().unwrap()),
                    current_speed: Some(target(1)),
                    target_speed: Some(target(1)),
                    // Live NBFC can report 0.0 as an automatic-mode placeholder.
                    requested_speed: Some(if fan_auto(1) { 0.0 } else { target(1) }),
                    speed_steps: Some(100),
                });
            }
            if let Some(hidden) = *self.hide_fan.lock().unwrap() {
                fans.retain(|fan| fan.index != hidden);
            }
            Ok(NbfcStatus {
                version: None,
                read_only: Some(false),
                selected_config: Some("test".into()),
                fans,
                warnings: Vec::new(),
            })
        }
        fn set_auto(&self) -> Result<crate::util::CommandOutput, String> {
            let call = self.auto_calls.fetch_add(1, Ordering::Relaxed) + 1;
            if self.fail_auto
                || self
                    .fail_auto_from_call
                    .lock()
                    .unwrap()
                    .is_some_and(|from| call >= from)
            {
                return Err("backend auto failure".into());
            }
            self.manual_targets.lock().unwrap().clear();
            Ok(ok_output())
        }
        fn set_auto_fan(&self, fan: u32) -> Result<crate::util::CommandOutput, String> {
            self.auto_fan_calls.fetch_add(1, Ordering::Relaxed);
            self.manual_targets.lock().unwrap().remove(&fan);
            Ok(ok_output())
        }
        fn set_speed(&self, fan: u32, percent: f64) -> Result<crate::util::CommandOutput, String> {
            self.set_speed_calls.fetch_add(1, Ordering::Relaxed);
            if *self.fail_speed_for.lock().unwrap() == Some(fan) {
                return Err(format!("fake backend rejected fan {fan}"));
            }
            if !self.ignore_speed {
                self.manual_targets.lock().unwrap().insert(fan, percent);
            }
            Ok(ok_output())
        }
        fn sensors_show(&self) -> Result<NbfcSensorConfig, String> {
            self.config_calls.fetch_add(1, Ordering::Relaxed);
            let mut fans = vec![NbfcFanSensor {
                index: 0,
                name: "CPU fan".into(),
                sensors: vec!["@CPU".into()],
                algorithm: Some("Average".into()),
            }];
            if self.two_cpu_fans {
                fans.push(NbfcFanSensor {
                    index: 1,
                    name: if self.gpu_second {
                        "GPU fan".into()
                    } else {
                        "CPU fan 2".into()
                    },
                    sensors: vec!["@CPU".into()],
                    algorithm: Some("Average".into()),
                });
            }
            Ok(NbfcSensorConfig { fans })
        }
        fn list_configs(&self) -> Result<Vec<String>, String> {
            Ok(vec!["test".into()])
        }
        fn rate_config_json(&self, name: &str) -> Result<String, String> {
            Ok(serde_json::json!({
                "config": name,
                "read_only": true,
                "writes": 0
            })
            .to_string())
        }
    }

    struct SpeedGate {
        entered: (Mutex<bool>, std::sync::Condvar),
        release: (Mutex<bool>, std::sync::Condvar),
    }

    impl SpeedGate {
        fn new() -> Self {
            Self {
                entered: (Mutex::new(false), std::sync::Condvar::new()),
                release: (Mutex::new(true), std::sync::Condvar::new()),
            }
        }

        fn block(&self) {
            *self.entered.0.lock().unwrap() = false;
            *self.release.0.lock().unwrap() = false;
        }

        fn wait_entered(&self) {
            let mut entered = self.entered.0.lock().unwrap();
            while !*entered {
                entered = self.entered.1.wait(entered).unwrap();
            }
        }

        fn release(&self) {
            *self.release.0.lock().unwrap() = true;
            self.release.1.notify_all();
        }
    }

    struct GatedBackend {
        inner: Arc<FakeBackend>,
        gate: Arc<SpeedGate>,
    }

    impl FanBackend for GatedBackend {
        fn version(&self) -> Result<NbfcVersion, String> {
            self.inner.version()
        }
        fn status(&self, version: NbfcVersion) -> Result<NbfcStatus, String> {
            self.inner.status(version)
        }
        fn set_auto(&self) -> Result<crate::util::CommandOutput, String> {
            self.inner.set_auto()
        }
        fn set_auto_fan(&self, fan: u32) -> Result<crate::util::CommandOutput, String> {
            self.inner.set_auto_fan(fan)
        }
        fn set_speed(&self, fan: u32, percent: f64) -> Result<crate::util::CommandOutput, String> {
            {
                let mut entered = self.gate.entered.0.lock().unwrap();
                *entered = true;
                self.gate.entered.1.notify_all();
            }
            {
                let mut release = self.gate.release.0.lock().unwrap();
                while !*release {
                    release = self.gate.release.1.wait(release).unwrap();
                }
            }
            self.inner.set_speed(fan, percent)
        }
        fn sensors_show(&self) -> Result<NbfcSensorConfig, String> {
            self.inner.sensors_show()
        }
        fn list_configs(&self) -> Result<Vec<String>, String> {
            self.inner.list_configs()
        }
    }

    fn ok_output() -> crate::util::CommandOutput {
        crate::util::CommandOutput {
            success: true,
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn snapshot() -> SensorSnapshot {
        let cpu = SensorSample::new(
            "cpu".into(),
            SensorRole::CpuPackage,
            SensorKind::Temperature,
            Some(55.0),
            Unit::Celsius,
            Some("55000".into()),
            "millidegree_c",
            SensorProvider::Hwmon,
            "coretemp".into(),
            None,
            Some("Package id 0".into()),
            None,
        );
        SensorSnapshot {
            sensors: vec![cpu],
            errors: Vec::new(),
            collected_at_ms: now_ms(),
        }
    }

    fn sysfs_with_cpu() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let hwmon = dir.path().join("class/hwmon/hwmon0");
        std::fs::create_dir_all(&hwmon).unwrap();
        std::fs::write(hwmon.join("name"), "coretemp\n").unwrap();
        std::fs::write(hwmon.join("temp1_input"), "55000\n").unwrap();
        std::fs::write(hwmon.join("temp1_label"), "Package id 0\n").unwrap();
        dir
    }

    fn sysfs_with_cpu_and_gpu() -> tempfile::TempDir {
        let dir = sysfs_with_cpu();
        let gpu = dir.path().join("class/hwmon/hwmon1");
        std::fs::create_dir_all(&gpu).unwrap();
        std::fs::write(gpu.join("name"), "amdgpu\n").unwrap();
        std::fs::write(gpu.join("temp1_input"), "55000\n").unwrap();
        std::fs::write(gpu.join("temp1_label"), "edge\n").unwrap();
        dir
    }

    #[test]
    fn refresh_caches_version_and_sensor_configuration() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        assert_eq!(backend.version_calls.load(Ordering::Relaxed), 1);
        assert_eq!(backend.config_calls.load(Ordering::Relaxed), 1);
        daemon.refresh();
        daemon.refresh();
        assert_eq!(backend.version_calls.load(Ordering::Relaxed), 1);
        assert_eq!(backend.config_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn version_refresh_triggers_on_service_restart_not_every_tick() {
        let version = Some(NbfcVersion {
            major: 0,
            minor: 5,
            patch: 3,
        });
        // Same service PID, fresh cache: keep the cached value.
        assert!(!version_needs_refresh(
            10_000,
            version,
            Some(100),
            9_000,
            Some(100),
            60_000
        ));
        // `nbfc_service` restarted: the client may have been replaced.
        assert!(version_needs_refresh(
            10_000,
            version,
            Some(100),
            9_000,
            Some(200),
            60_000
        ));
        // Cache aged out even without a visible PID change.
        assert!(version_needs_refresh(
            70_000,
            version,
            Some(100),
            9_000,
            Some(100),
            60_000
        ));
        // Nothing cached yet.
        assert!(version_needs_refresh(
            10_000,
            None,
            None,
            0,
            Some(100),
            60_000
        ));
    }

    #[test]
    fn protected_operation_reprobes_version_after_backend_upgrade() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let nbfc_socket = dir.path().join("nbfc.sock");
        let _listener = UnixListener::bind(&nbfc_socket).unwrap();
        std::fs::set_permissions(&nbfc_socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let backend = Arc::new(FakeBackend::with_version(0, 5, 3));
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let config = DaemonConfig {
            nbfc_socket: nbfc_socket.clone(),
            ..DaemonConfig::default()
        };
        let daemon = Daemon::new(backend_trait, SysfsRoot::new(sysfs.path()), config);
        assert!(!daemon.status().capabilities.unwrap().protected_manual);

        // Simulate an in-place NBFC client/service upgrade after startup.
        backend.set_version(NbfcVersion {
            major: 0,
            minor: 5,
            patch: 4,
        });

        let request = Request {
            id: 21,
            method: "fan.set".into(),
            params: serde_json::json!({
                "fan": 0, "percent": 50.0, "confirm": true, "dry_run": true
            }),
        };
        let response = daemon.handle_request(&request);
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
        assert!(
            daemon
                .status()
                .capabilities
                .expect("capabilities")
                .protected_manual
        );
    }

    #[test]
    fn profile_selection_is_simulated_and_never_writes_fans() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 7,
            method: "profile.select".into(),
            params: serde_json::json!({"name":"quiet"}),
        };
        let response = daemon.handle_request(&request);
        assert!(response.ok, "{:?}", response.error);
        let result = response.result.expect("result");
        assert_eq!(result["hardware_applied"], false);
        assert_eq!(result["profile"]["mode"], "simulated");
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
        let status = daemon.status();
        assert_eq!(status.profile.selected.as_deref(), Some("quiet"));
        assert_eq!(status.profile.mode, Some(RegulationMode::Simulated));
        assert_eq!(status.profile.targets[0].target_percent, Some(30.0));
    }

    #[test]
    fn concurrent_refresh_never_loses_a_simulated_selection() {
        use std::sync::atomic::AtomicBool;
        use std::thread;

        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Arc::new(Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig {
                startup_safe_auto: false,
                ..DaemonConfig::default()
            },
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let refresher = Arc::clone(&daemon);
        let stop_flag = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                refresher.refresh();
                thread::yield_now();
            }
        });

        for id in 0..10 {
            let request = Request {
                id,
                method: "profile.select".into(),
                params: serde_json::json!({"name":"quiet"}),
            };
            let response = daemon.handle_request(&request);
            assert!(response.ok, "{:?}", response.error);
            let result = response.result.expect("result");
            assert_eq!(result["profile"]["mode"], "simulated");
            // A refresh that snapshotted the previous selection must not commit
            // a default profile over the new selection.
            assert_eq!(
                daemon.status().profile.mode,
                Some(RegulationMode::Simulated)
            );
        }

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn selected_profile_is_re_evaluated_after_sensor_change() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 1,
            method: "profile.select".into(),
            params: serde_json::json!({"name":"quiet"}),
        };
        assert!(daemon.handle_request(&request).ok);
        let first = daemon.status().profile.targets[0].target_percent.unwrap();
        std::fs::write(
            sysfs.path().join("class/hwmon/hwmon0/temp1_input"),
            "75000
",
        )
        .unwrap();
        daemon.refresh();
        let second = daemon.status().profile.targets[0].target_percent.unwrap();
        assert!(
            second > first,
            "target must follow fresh telemetry: {first} -> {second}"
        );
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn profile_selection_rejects_curveless_or_unknown_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = Daemon::new(backend, SysfsRoot::new(dir.path()), DaemonConfig::default());
        for name in ["safe-auto", "custom", "not-a-profile"] {
            let request = Request {
                id: 1,
                method: "profile.select".into(),
                params: serde_json::json!({"name": name}),
            };
            let response = daemon.handle_request(&request);
            assert!(!response.ok, "{name}");
        }
    }

    fn custom_profile_value(name: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "title": "Custom test",
            "description": "test curve",
            "points": [
                {"temperature_c": 40.0, "percent": 25.0},
                {"temperature_c": 80.0, "percent": 80.0}
            ],
            "min_percent": 20.0,
            "max_percent": 100.0,
            "hysteresis_c": 3.0,
            "max_step_percent": 10.0,
            "stale_after_ms": 3000,
            "fan_roles": ["cpu"]
        })
    }

    fn daemon_with_custom_store(sysfs: &tempfile::TempDir, dir: &tempfile::TempDir) -> Arc<Daemon> {
        let backend: Arc<dyn FanBackend> = Arc::new(FakeBackend::new());
        let config = DaemonConfig {
            custom_profiles_path: dir.path().join("profiles.json"),
            ..DaemonConfig::default()
        };
        Arc::new(Daemon::new(backend, SysfsRoot::new(sysfs.path()), config))
    }

    fn daemon_with_backend(
        backend: Arc<dyn FanBackend>,
        sysfs: &tempfile::TempDir,
        dir: &tempfile::TempDir,
    ) -> Arc<Daemon> {
        let config = DaemonConfig {
            startup_safe_auto: false,
            custom_profiles_path: dir.path().join("profiles.json"),
            ..DaemonConfig::default()
        };
        Arc::new(Daemon::new(backend, SysfsRoot::new(sysfs.path()), config))
    }

    fn apply_profile_request(name: &str, dry_run: bool) -> Request {
        Request {
            id: 400,
            method: "profile.apply".into(),
            params: serde_json::json!({
                "name": name,
                "confirm": true,
                "dry_run": dry_run,
            }),
        }
    }

    fn set_many_request(targets: serde_json::Value) -> Request {
        Request {
            id: 300,
            method: "fan.set_many".into(),
            params: serde_json::json!({
                "targets": targets,
                "confirm": true,
                "dry_run": false,
            }),
        }
    }

    #[test]
    fn connection_gate_bounds_concurrent_ipc_handlers() {
        let gate = Arc::new(ConnectionGate::new(2));
        let first = gate.try_enter().expect("first permit");
        let second = gate.try_enter().expect("second permit");
        assert_eq!(gate.active(), 2);
        assert!(gate.try_enter().is_none());
        drop(first);
        assert_eq!(gate.active(), 1);
        let third = gate.try_enter().expect("permit after release");
        drop(second);
        drop(third);
        assert_eq!(gate.active(), 0);
    }

    #[test]
    fn repeated_status_queries_and_refreshes_are_stable_and_never_write() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = Arc::new(Daemon::new(
            backend.clone(),
            SysfsRoot::new(sysfs.path()),
            DaemonConfig {
                startup_safe_auto: false,
                custom_profiles_path: dir.path().join("profiles.json"),
                ..DaemonConfig::default()
            },
        ));
        let mut sensor_count = None;
        for i in 0..200 {
            if i % 20 == 0 {
                daemon.refresh();
            }
            let response = daemon.handle_request(&Request {
                id: i,
                method: "status.get".into(),
                params: serde_json::json!({}),
            });
            assert!(response.ok, "{:?}", response.error);
            let value = response.result.unwrap();
            assert_eq!(value["nbfc"]["fans"].as_array().unwrap().len(), 1);
            let count = value["sensors"]["sensors"].as_array().unwrap().len();
            assert!(count >= 1);
            assert_eq!(*sensor_count.get_or_insert(count), count);
            assert_eq!(value["safety"]["state"], "safe_auto");
        }
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
        assert_eq!(backend.auto_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn concurrent_status_queries_are_consistent_and_never_write() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = Arc::new(Daemon::new(
            backend.clone(),
            SysfsRoot::new(sysfs.path()),
            DaemonConfig {
                startup_safe_auto: false,
                custom_profiles_path: dir.path().join("profiles.json"),
                ..DaemonConfig::default()
            },
        ));
        let mut handles = Vec::new();
        for worker in 0..8 {
            let daemon = Arc::clone(&daemon);
            handles.push(thread::spawn(move || {
                for i in 0..25 {
                    let response = daemon.handle_request(&Request {
                        id: (worker * 100 + i) as u64,
                        method: "status.get".into(),
                        params: serde_json::json!({}),
                    });
                    assert!(response.ok, "{:?}", response.error);
                    let value = response.result.unwrap();
                    assert_eq!(value["nbfc"]["fans"].as_array().unwrap().len(), 1);
                    assert_eq!(value["safety"]["state"], "safe_auto");
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
        assert_eq!(backend.auto_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn unknown_ipc_method_is_a_typed_protocol_error() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let response = daemon.handle_request(&Request {
            id: 91,
            method: "does.not.exist".into(),
            params: serde_json::json!({}),
        });
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "unknown_method");
    }

    #[test]
    fn missing_acpiexec_is_an_actionable_analysis_limitation() {
        let error = hardware_analysis_error(
            "nbfc rate-config: ERROR: nbfc: acpiexec: No such file or directory".to_string(),
        );
        assert_eq!(error.code, "analysis_unavailable");
        assert!(error.message.contains("acpiexec"));
        assert!(error.message.contains("no fan write"));
        assert_eq!(error.details.len(), 2);
    }

    #[test]
    fn set_many_verifies_both_fans_and_records_manual_leases() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&set_many_request(serde_json::json!([
            {"fan": 0, "percent": 40.0},
            {"fan": 1, "percent": 40.0}
        ])));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["verified"], true);
        assert_eq!(value["partial"], false);
        assert_eq!(value["results"].as_array().unwrap().len(), 2);
        assert_eq!(value["results"][0]["verified"], true);
        assert_eq!(value["results"][1]["verified"], true);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 2);
        let status = daemon.status();
        assert_eq!(status.safety.state, SafetyState::Manual);
        assert_eq!(status.safety.preset.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn set_many_partial_backend_failure_is_reported_and_recovers_auto() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_failing_second_fan());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&set_many_request(serde_json::json!([
            {"fan": 0, "percent": 40.0},
            {"fan": 1, "percent": 50.0}
        ])));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["verified"], false);
        assert_eq!(value["partial"], true);
        assert_eq!(value["automatic_recovery"], true);
        assert_eq!(value["results"][0]["verified"], true);
        assert_eq!(value["results"][1]["verified"], false);
        assert!(
            value["results"][1]["error"]
                .as_str()
                .unwrap()
                .contains("rejected")
        );
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
        let status = daemon.status();
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(status.nbfc.unwrap().all_auto());
    }

    #[test]
    fn manual_enters_at_current_targets_and_auto_fan_leaves_other_leases() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let manual = daemon.handle_request(&Request {
            id: 301,
            method: "fan.manual".into(),
            params: serde_json::json!({"confirm": true, "dry_run": false}),
        });
        assert!(manual.ok, "{:?}", manual.error);
        let manual_value = manual.result.unwrap();
        assert_eq!(manual_value["verified"], true);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 2);
        assert_eq!(daemon.status().safety.state, SafetyState::Manual);

        let auto_one = daemon.handle_request(&Request {
            id: 302,
            method: "fan.auto".into(),
            params: serde_json::json!({"fan": 0}),
        });
        assert!(auto_one.ok, "{:?}", auto_one.error);
        assert_eq!(backend.auto_fan_calls.load(Ordering::Relaxed), 1);
        let status = daemon.status();
        assert_eq!(status.safety.state, SafetyState::Manual);
        assert_eq!(status.safety.preset.as_ref().unwrap().len(), 1);
        assert_eq!(
            status.nbfc.as_ref().unwrap().fan(0).unwrap().auto_mode,
            Some(true)
        );
        assert_eq!(
            status.nbfc.as_ref().unwrap().fan(1).unwrap().auto_mode,
            Some(false)
        );

        let auto_rest = daemon.handle_request(&Request {
            id: 303,
            method: "fan.auto".into(),
            params: serde_json::json!({"fan": 1}),
        });
        assert!(auto_rest.ok, "{:?}", auto_rest.error);
        assert_eq!(daemon.status().safety.state, SafetyState::SafeAuto);
    }

    #[test]
    fn separate_manual_requests_keep_all_fans_under_leases() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        for (id, fan, percent) in [(430, 0, 50.0), (431, 1, 60.0)] {
            let response = daemon.handle_request(&Request {
                id,
                method: "fan.set".into(),
                params: serde_json::json!({"fan": fan, "percent": percent, "confirm": true}),
            });
            assert!(response.ok, "{:?}", response.error);
        }
        let status = daemon.status();
        assert_eq!(status.safety.state, SafetyState::Manual);
        let leases = status.safety.preset.as_ref().unwrap();
        assert_eq!(leases.len(), 2);
        assert!(leases.iter().any(|lease| lease.fan_index == 0));
        assert!(leases.iter().any(|lease| lease.fan_index == 1));
    }

    #[test]
    fn set_many_dry_run_never_writes() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&Request {
            id: 304,
            method: "fan.set_many".into(),
            params: serde_json::json!({
                "targets": [{"fan": 0, "percent": 50.0}],
                "confirm": true,
                "dry_run": true,
            }),
        });
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["dry_run"], true);
        assert_eq!(value["verified"], false);
        assert_eq!(value["results"][0]["requested_percent"], 50.0);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
        assert_eq!(backend.auto_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn set_many_rejects_duplicate_and_out_of_range_targets_before_writing() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let duplicate = daemon.handle_request(&set_many_request(serde_json::json!([
            {"fan": 0, "percent": 40.0},
            {"fan": 0, "percent": 50.0}
        ])));
        assert!(!duplicate.ok);
        assert_eq!(duplicate.error.unwrap().code, "bad_request");
        let range = daemon.handle_request(&set_many_request(serde_json::json!([
            {"fan": 0, "percent": 101.0}
        ])));
        assert!(!range.ok);
        assert_eq!(range.error.unwrap().code, "bad_request");
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn readback_disagreement_is_unverified_and_recovers_auto() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_ignored_target());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&set_many_request(serde_json::json!([
            {"fan": 0, "percent": 60.0}
        ])));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["verified"], false);
        assert_eq!(value["partial"], false);
        assert_eq!(value["automatic_recovery"], true);
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
        assert_eq!(daemon.status().safety.state, SafetyState::FailSafe);
    }

    #[test]
    fn profile_apply_activates_closed_loop_for_eligible_fans() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["hardware_applied"], true);
        assert_eq!(value["verified"], true);
        assert_eq!(value["partial"], false);
        assert_eq!(value["applied_fans"], serde_json::json!([0, 1]));
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 2);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.mode, Some(RegulationMode::Active));
        assert_eq!(status.safety.state, SafetyState::Profile);
        daemon.tick();
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn profile_apply_uses_independent_gpu_temperature() {
        let sysfs = sysfs_with_cpu_and_gpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_cpu_and_gpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["hardware_applied"], true);
        assert_eq!(value["verified"], true);
        assert_eq!(value["partial"], false, "{value}");
        assert_eq!(value["applied_fans"], serde_json::json!([0, 1]));
        assert_eq!(value["blocked_fans"], serde_json::json!([]), "{value}");
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn profile_apply_reports_partial_fans_and_keeps_blocked_fan_out() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_cpu_and_gpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["hardware_applied"], true);
        assert_eq!(value["verified"], true);
        assert_eq!(value["partial"], true);
        assert_eq!(value["applied_fans"], serde_json::json!([0]));
        assert_eq!(value["blocked_fans"], serde_json::json!([1]));
        assert!(
            value["blocked"][0]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|reason| reason.as_str().unwrap_or("").contains("@CPU")),
            "{value}"
        );
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn profile_apply_returns_blocked_manual_fan_to_automatic() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_cpu_and_gpu_fans());
        backend.set_speed(1, 50.0).unwrap();
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert_eq!(
            daemon
                .status()
                .nbfc
                .as_ref()
                .unwrap()
                .fan(1)
                .unwrap()
                .auto_mode,
            Some(false)
        );
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(
            response.result.unwrap()["blocked_fans"],
            serde_json::json!([1])
        );
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
        assert_eq!(
            daemon
                .status()
                .nbfc
                .as_ref()
                .unwrap()
                .fan(1)
                .unwrap()
                .auto_mode,
            Some(true)
        );
        assert!(daemon.status().profile.active);
    }

    #[test]
    fn profile_tick_updates_target_from_fresh_temperature() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let baseline_calls = backend.set_speed_calls.load(Ordering::Relaxed);
        std::fs::write(
            sysfs.path().join("class/hwmon/hwmon0/temp1_input"),
            "80000\n",
        )
        .unwrap();
        daemon.refresh();
        daemon.tick();
        assert!(backend.set_speed_calls.load(Ordering::Relaxed) > baseline_calls);
        let profile = daemon.status().profile;
        assert!(profile.active);
        assert!(
            profile
                .targets
                .iter()
                .any(|target| target.target_percent.unwrap_or(0.0) > 30.0)
        );
    }

    #[test]
    fn sampler_cannot_overwrite_newer_manual_request() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let inner = Arc::new(FakeBackend::new());
        let gate = Arc::new(SpeedGate::new());
        let backend: Arc<dyn FanBackend> = Arc::new(GatedBackend {
            inner: inner.clone(),
            gate: gate.clone(),
        });
        let daemon = daemon_with_backend(backend, &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        std::fs::write(
            sysfs.path().join("class/hwmon/hwmon0/temp1_input"),
            "80000\n",
        )
        .unwrap();
        daemon.refresh();
        gate.block();
        let tick_daemon = daemon.clone();
        let tick = std::thread::spawn(move || tick_daemon.tick());
        gate.wait_entered();

        let (tx, rx) = std::sync::mpsc::channel();
        let manual_daemon = daemon.clone();
        let manual = std::thread::spawn(move || {
            let response = manual_daemon.handle_request(&Request {
                id: 420,
                method: "fan.set".into(),
                params: serde_json::json!({"fan": 0, "percent": 60.0, "confirm": true}),
            });
            tx.send(response).unwrap();
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(200)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        gate.release();
        tick.join().unwrap();
        let response = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        manual.join().unwrap();
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::Manual);
        assert_eq!(
            inner.manual_targets.lock().unwrap().get(&0).copied(),
            Some(60.0)
        );
    }

    #[test]
    fn sampler_cannot_overwrite_newer_profile_application() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let inner = Arc::new(FakeBackend::new());
        let gate = Arc::new(SpeedGate::new());
        let backend: Arc<dyn FanBackend> = Arc::new(GatedBackend {
            inner: inner.clone(),
            gate: gate.clone(),
        });
        let daemon = daemon_with_backend(backend, &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        std::fs::write(
            sysfs.path().join("class/hwmon/hwmon0/temp1_input"),
            "80000\n",
        )
        .unwrap();
        daemon.refresh();
        gate.block();
        let tick_daemon = daemon.clone();
        let tick = std::thread::spawn(move || tick_daemon.tick());
        gate.wait_entered();

        let (tx, rx) = std::sync::mpsc::channel();
        let apply_daemon = daemon.clone();
        let apply = std::thread::spawn(move || {
            let response = apply_daemon.handle_request(&apply_profile_request("balanced", false));
            tx.send(response).unwrap();
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(200)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        gate.release();
        tick.join().unwrap();
        let response = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        apply.join().unwrap();
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.selected.as_deref(), Some("balanced"));
        let target = inner
            .manual_targets
            .lock()
            .unwrap()
            .get(&0)
            .copied()
            .unwrap();
        assert!(
            target > 80.0,
            "final target was {target}, expected balanced"
        );
    }

    #[test]
    fn profile_backend_failure_recovers_all_fans_to_auto() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_failing_second_fan());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["hardware_applied"], false);
        assert_eq!(value["verified"], false);
        assert_eq!(value["partial"], false);
        assert_eq!(value["applied_fans"], serde_json::json!([]));
        assert_eq!(value["verified_before_recovery"], serde_json::json!([0]));
        assert_eq!(value["automatic_recovery"], true);
        assert!(!value["failures"].as_array().unwrap().is_empty());
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
        assert!(
            daemon
                .status()
                .nbfc
                .as_ref()
                .unwrap()
                .fans
                .iter()
                .all(|fan| fan.auto_mode == Some(true))
        );
    }

    #[test]
    fn per_fan_auto_keeps_profile_for_remaining_fans() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let response = daemon.handle_request(&Request {
            id: 410,
            method: "fan.auto".into(),
            params: serde_json::json!({"fan": 0}),
        });
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.applied_fans, vec![1]);
        assert_eq!(status.safety.state, SafetyState::Profile);
        assert_eq!(
            status.nbfc.as_ref().unwrap().fan(0).unwrap().auto_mode,
            Some(true)
        );
        assert_eq!(
            status.nbfc.as_ref().unwrap().fan(1).unwrap().auto_mode,
            Some(false)
        );

        let response = daemon.handle_request(&Request {
            id: 411,
            method: "fan.auto".into(),
            params: serde_json::json!({"fan": 1}),
        });
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::SafeAuto);
        assert!(status.nbfc.as_ref().unwrap().all_auto());
    }

    #[test]
    fn profile_apply_takes_ownership_of_unrelated_manual_fans() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_cpu_and_gpu_fans());
        backend.set_speed(1, 50.0).unwrap();
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let create = Request {
            id: 412,
            method: "custom.create".into(),
            params: serde_json::json!({"profile": custom_profile_value("cpu-only")}),
        };
        assert!(daemon.handle_request(&create).ok);
        let response = daemon.handle_request(&apply_profile_request("cpu-only", false));
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.applied_fans, vec![0]);
        assert_eq!(
            status.nbfc.as_ref().unwrap().fan(1).unwrap().auto_mode,
            Some(true)
        );
    }

    #[test]
    fn profile_recovery_failure_is_not_reported_as_success() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_failing_second_fan());
        *backend.fail_auto_from_call.lock().unwrap() = Some(2);
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["hardware_applied"], false);
        assert_eq!(value["automatic_recovery"], false);
        assert_eq!(value["recovery_failed"], true);
        assert!(
            value["message"]
                .as_str()
                .unwrap()
                .contains("automatic recovery failed")
        );
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(
            status
                .safety
                .reasons
                .iter()
                .any(|reason| reason.contains("automatic recovery failed"))
        );
    }

    #[test]
    fn profile_target_mismatch_recovers_and_never_reports_active() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_ignored_target());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["hardware_applied"], false);
        assert_eq!(value["automatic_recovery"], true);
        assert_eq!(value["recovery_failed"], false);
        assert!(!value["failures"].as_array().unwrap().is_empty());
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(status.nbfc.as_ref().unwrap().all_auto());
    }

    #[test]
    fn deleting_an_active_custom_profile_returns_fans_to_auto() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let create = Request {
            id: 413,
            method: "custom.create".into(),
            params: serde_json::json!({"profile": custom_profile_value("delete-me")}),
        };
        assert!(daemon.handle_request(&create).ok);
        assert!(
            daemon
                .handle_request(&apply_profile_request("delete-me", false))
                .ok
        );
        assert!(daemon.status().profile.active);
        let response = daemon.handle_request(&Request {
            id: 414,
            method: "custom.delete".into(),
            params: serde_json::json!({"name": "delete-me", "confirm": true}),
        });
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::SafeAuto);
        assert!(status.nbfc.as_ref().unwrap().all_auto());
    }

    #[test]
    fn invalid_active_custom_edit_does_not_disturb_regulation() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let create = Request {
            id: 415,
            method: "custom.create".into(),
            params: serde_json::json!({"profile": custom_profile_value("edit-me")}),
        };
        assert!(daemon.handle_request(&create).ok);
        assert!(
            daemon
                .handle_request(&apply_profile_request("edit-me", false))
                .ok
        );
        let mut invalid = custom_profile_value("edit-me");
        invalid["points"] = serde_json::json!([
            {"temperature_c": 80.0, "percent": 80.0},
            {"temperature_c": 40.0, "percent": 25.0}
        ]);
        let response = daemon.handle_request(&Request {
            id: 416,
            method: "custom.update".into(),
            params: serde_json::json!({"profile": invalid}),
        });
        assert!(!response.ok);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.selected.as_deref(), Some("edit-me"));
        assert_eq!(status.safety.state, SafetyState::Profile);
    }

    #[test]
    fn profile_stops_when_sensor_snapshot_is_stale() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        {
            let mut state = daemon.state.lock().unwrap();
            state.sensors.collected_at_ms = now_ms().saturating_sub(60_000);
        }
        daemon.tick();
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(
            status
                .safety
                .reasons
                .iter()
                .any(|reason| reason.contains("stale"))
        );
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn profile_sensor_loss_stops_regulation_and_recovers_auto() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        std::fs::remove_file(sysfs.path().join("class/hwmon/hwmon0/temp1_input")).unwrap();
        daemon.refresh();
        daemon.tick();
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn profile_stops_when_an_applied_fan_disappears() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        *backend.hide_fan.lock().unwrap() = Some(1);
        daemon.refresh();
        daemon.tick();
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(
            status
                .safety
                .reasons
                .iter()
                .any(|reason| reason.contains("fan 1"))
        );
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn profile_stops_when_nbfc_reports_critical_mode() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        *backend.critical.lock().unwrap() = true;
        daemon.refresh();
        daemon.tick();
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(
            status
                .safety
                .reasons
                .iter()
                .any(|reason| reason.contains("critical mode"))
        );
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
        assert!(status.nbfc.as_ref().unwrap().all_auto());
    }

    #[test]
    fn dry_run_takeover_does_not_deactivate_active_profile() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let dry = daemon.handle_request(&Request {
            id: 403,
            method: "fan.set".into(),
            params: serde_json::json!({
                "fan": 0, "percent": 60.0, "confirm": true, "dry_run": true
            }),
        });
        assert!(dry.ok, "{:?}", dry.error);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.safety.state, SafetyState::Profile);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn manual_control_takes_ownership_from_active_profile() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let manual = daemon.handle_request(&Request {
            id: 401,
            method: "fan.set".into(),
            params: serde_json::json!({"fan": 0, "percent": 60.0, "confirm": true}),
        });
        assert!(manual.ok, "{:?}", manual.error);
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::Manual);
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn profile_clear_returns_fans_to_auto() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let response = daemon.handle_request(&Request {
            id: 402,
            method: "profile.clear".into(),
            params: serde_json::json!({}),
        });
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::SafeAuto);
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn custom_profile_applies_to_its_selected_roles() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let create = Request {
            id: 404,
            method: "custom.create".into(),
            params: serde_json::json!({"profile": custom_profile_value("cpu-curve")}),
        };
        assert!(daemon.handle_request(&create).ok);
        let response = daemon.handle_request(&apply_profile_request("cpu-curve", false));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["hardware_applied"], true);
        assert_eq!(value["verified"], true);
        assert_eq!(value["applied_fans"], serde_json::json!([0]));
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.selected.as_deref(), Some("cpu-curve"));
    }

    #[test]
    fn profile_with_no_eligible_fans_writes_nothing() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        std::fs::remove_file(sysfs.path().join("class/hwmon/hwmon0/temp1_input")).unwrap();
        daemon.refresh();
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "unsafe_refused");
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
        assert!(!daemon.status().profile.active);
    }

    #[test]
    fn profile_dry_run_never_writes_or_activates() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("quiet", true));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["hardware_applied"], false);
        assert_eq!(value["verified"], false);
        assert_eq!(value["dry_run"], true);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
        assert!(!daemon.status().profile.active);
    }

    #[test]
    fn safe_auto_dry_run_never_writes() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        let response = daemon.handle_request(&apply_profile_request("safe-auto", true));
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["action"], "safe_auto");
        assert_eq!(value["dry_run"], true);
        assert_eq!(value["hardware_applied"], false);
        assert_eq!(backend.auto_calls.load(Ordering::Relaxed), 0);
        assert_eq!(daemon.status().safety.state, SafetyState::SafeAuto);
    }

    #[test]
    fn simulation_selection_is_rejected_while_profile_is_active() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let response = daemon.handle_request(&Request {
            id: 450,
            method: "profile.select".into(),
            params: serde_json::json!({"name": "balanced"}),
        });
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "profile_active");
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.selected.as_deref(), Some("quiet"));
        assert_eq!(status.safety.state, SafetyState::Profile);
    }

    #[test]
    fn profile_dry_run_while_active_keeps_it_active() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let auto_before = backend.auto_calls.load(Ordering::Relaxed);
        let response = daemon.handle_request(&apply_profile_request("balanced", true));
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.result.unwrap()["dry_run"], true);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.selected.as_deref(), Some("quiet"));
        assert_eq!(status.safety.state, SafetyState::Profile);
        assert_eq!(backend.auto_calls.load(Ordering::Relaxed), auto_before);
    }

    #[test]
    fn rejected_profile_request_keeps_active_profile() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let auto_before = backend.auto_calls.load(Ordering::Relaxed);
        std::fs::remove_file(sysfs.path().join("class/hwmon/hwmon0/temp1_input")).unwrap();
        daemon.refresh();
        let response = daemon.handle_request(&apply_profile_request("balanced", false));
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "unsafe_refused");
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.selected.as_deref(), Some("quiet"));
        assert_eq!(status.safety.state, SafetyState::Profile);
        assert_eq!(backend.auto_calls.load(Ordering::Relaxed), auto_before);
    }

    #[test]
    fn profile_is_replaced_by_preset_and_all_auto() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let preset = daemon.handle_request(&Request {
            id: 440,
            method: "preset.apply".into(),
            params: serde_json::json!({"name": "performance", "confirm": true}),
        });
        assert!(preset.ok, "{:?}", preset.error);
        let mut status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::Manual);
        assert_eq!(
            status.nbfc.as_ref().unwrap().fan(0).unwrap().target_speed,
            Some(80.0)
        );

        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.safety.state, SafetyState::Profile);

        let all_auto = daemon.handle_request(&Request {
            id: 441,
            method: "fan.auto".into(),
            params: serde_json::json!({}),
        });
        assert!(all_auto.ok, "{:?}", all_auto.error);
        status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::SafeAuto);
        assert!(status.nbfc.as_ref().unwrap().all_auto());
    }

    #[test]
    fn repeated_profile_application_keeps_one_clean_owner() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.applied_fans, vec![0, 1]);
        assert_eq!(status.safety.state, SafetyState::Profile);
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 2);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn profile_switch_replaces_regulation_ownership() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let response = daemon.handle_request(&apply_profile_request("balanced", false));
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.profile.selected.as_deref(), Some("balanced"));
        assert_eq!(status.safety.state, SafetyState::Profile);
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn profile_can_be_reapplied_after_sensor_recovery() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(FakeBackend::new());
        let daemon = daemon_with_backend(backend.clone(), &sysfs, &dir);
        assert!(
            daemon
                .handle_request(&apply_profile_request("quiet", false))
                .ok
        );
        let temperature = sysfs.path().join("class/hwmon/hwmon0/temp1_input");
        let original = std::fs::read_to_string(&temperature).unwrap();
        std::fs::remove_file(&temperature).unwrap();
        daemon.refresh();
        daemon.tick();
        assert!(!daemon.status().profile.active);
        assert_eq!(daemon.status().safety.state, SafetyState::FailSafe);
        std::fs::write(&temperature, original).unwrap();
        daemon.refresh();
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert!(status.profile.active);
        assert_eq!(status.safety.state, SafetyState::Profile);
    }

    #[test]
    fn hardware_validate_is_executed_by_the_supervisor_backend() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let request = Request {
            id: 77,
            method: "hardware.validate".into(),
            params: serde_json::json!({"name": "test"}),
        };
        let response = daemon.handle_request(&request);
        assert!(response.ok, "{:?}", response.error);
        let value = response.result.unwrap();
        assert_eq!(value["schema"], "neosense.hardware_validation/v1");
        assert_eq!(value["name"], "test");
        assert_eq!(value["analysis"]["read_only"], true);
        assert_eq!(value["hardware_applied"], false);
    }

    #[test]
    fn custom_profile_is_created_selected_and_never_writes() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let create = Request {
            id: 70,
            method: "custom.create".into(),
            params: serde_json::json!({"profile": custom_profile_value("desk")}),
        };
        let response = daemon.handle_request(&create);
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.result.unwrap()["action"], "created");
        let status = daemon.status();
        assert_eq!(status.custom.profiles.len(), 1);
        assert_eq!(status.custom.profiles[0].name, "desk");

        let select = Request {
            id: 71,
            method: "profile.select".into(),
            params: serde_json::json!({"name": "desk"}),
        };
        let response = daemon.handle_request(&select);
        assert!(response.ok, "{:?}", response.error);
        let status = daemon.status();
        assert_eq!(status.profile.selected.as_deref(), Some("desk"));
        assert_eq!(status.profile.mode, Some(RegulationMode::Simulated));
        assert_eq!(status.profile.targets.len(), 1);
        assert_eq!(status.profile.targets[0].fan_name, "CPU fan");
        assert!(status.profile.targets[0].available);
        assert!(status.profile.targets[0].target_percent.is_some());
        assert_eq!(status.custom.profiles[0].fan_roles, vec![FanRole::Cpu]);
    }

    #[test]
    fn custom_profiles_survive_a_supervisor_restart() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        {
            let daemon = daemon_with_custom_store(&sysfs, &dir);
            let create = Request {
                id: 75,
                method: "custom.create".into(),
                params: serde_json::json!({"profile": custom_profile_value("persisted")}),
            };
            assert!(daemon.handle_request(&create).ok);
        }
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let status = daemon.status();
        assert_eq!(status.custom.profiles.len(), 1);
        assert_eq!(status.custom.profiles[0].name, "persisted");
        let select = Request {
            id: 76,
            method: "profile.select".into(),
            params: serde_json::json!({"name": "persisted"}),
        };
        assert!(daemon.handle_request(&select).ok);
        assert_eq!(
            daemon.status().profile.selected.as_deref(),
            Some("persisted")
        );
    }

    #[test]
    fn custom_profile_crud_is_supervisor_owned() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let create = Request {
            id: 80,
            method: "custom.create".into(),
            params: serde_json::json!({"profile": custom_profile_value("desk")}),
        };
        assert!(daemon.handle_request(&create).ok);

        let duplicate = Request {
            id: 81,
            method: "custom.duplicate".into(),
            params: serde_json::json!({"from": "desk", "to": "desk-copy"}),
        };
        assert!(daemon.handle_request(&duplicate).ok);
        let rename = Request {
            id: 82,
            method: "custom.rename".into(),
            params: serde_json::json!({"from": "desk-copy", "to": "desk-renamed"}),
        };
        assert!(daemon.handle_request(&rename).ok);
        assert_eq!(daemon.status().custom.profiles.len(), 2);

        let no_confirm = Request {
            id: 83,
            method: "custom.delete".into(),
            params: serde_json::json!({"name": "desk"}),
        };
        let response = daemon.handle_request(&no_confirm);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "confirmation_required");
        assert_eq!(daemon.status().custom.profiles.len(), 2);

        let delete = Request {
            id: 84,
            method: "custom.delete".into(),
            params: serde_json::json!({"name": "desk", "confirm": true}),
        };
        assert!(daemon.handle_request(&delete).ok);
        assert_eq!(daemon.status().custom.profiles.len(), 1);
    }

    #[test]
    fn renaming_selected_custom_curve_keeps_the_simulation_selection_valid() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let create = Request {
            id: 95,
            method: "custom.create".into(),
            params: serde_json::json!({"profile": custom_profile_value("rename-me")}),
        };
        assert!(daemon.handle_request(&create).ok);
        let select = Request {
            id: 96,
            method: "profile.select".into(),
            params: serde_json::json!({"name": "rename-me"}),
        };
        assert!(daemon.handle_request(&select).ok);
        let rename = Request {
            id: 97,
            method: "custom.rename".into(),
            params: serde_json::json!({"from": "rename-me", "to": "renamed"}),
        };
        assert!(daemon.handle_request(&rename).ok);
        let status = daemon.status();
        assert_eq!(status.profile.selected.as_deref(), Some("renamed"));
        assert_eq!(status.profile.mode, Some(RegulationMode::Simulated));
    }

    #[test]
    fn deleting_selected_custom_curve_clears_the_simulation_selection() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let create = Request {
            id: 100,
            method: "custom.create".into(),
            params: serde_json::json!({"profile": custom_profile_value("delete-me")}),
        };
        assert!(daemon.handle_request(&create).ok);
        let select = Request {
            id: 101,
            method: "profile.select".into(),
            params: serde_json::json!({"name": "delete-me"}),
        };
        assert!(daemon.handle_request(&select).ok);
        let delete = Request {
            id: 102,
            method: "custom.delete".into(),
            params: serde_json::json!({"name": "delete-me", "confirm": true}),
        };
        assert!(daemon.handle_request(&delete).ok);
        let status = daemon.status();
        assert!(status.profile.selected.is_none());
        assert!(status.custom.profiles.is_empty());
    }

    #[test]
    fn a_near_limit_custom_store_keeps_status_below_the_ipc_message_limit() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let store = crate::custom_profiles::Store::new(dir.path().join("profiles.json"));
        for index in 0..20 {
            let points = (0..12)
                .map(|point| crate::regulation::CurvePoint {
                    temperature_c: 30.0 + f64::from(point) * 5.0,
                    percent: 20.0 + f64::from(point) * 5.0,
                })
                .collect();
            store
                .create(CustomProfile {
                    name: format!("curve-{index}"),
                    title: format!("Curve {index}"),
                    description: "x".repeat(500),
                    points,
                    min_percent: 20.0,
                    max_percent: 100.0,
                    hysteresis_c: 3.0,
                    max_step_percent: 10.0,
                    stale_after_ms: 3_000,
                    fan_roles: vec![FanRole::Cpu],
                })
                .unwrap();
        }
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        let status = daemon.status();
        assert_eq!(status.custom.profiles.len(), 20);
        let encoded = serde_json::to_vec(&status).unwrap();
        assert!(
            encoded.len() < crate::ipc::MAX_MESSAGE,
            "status is {} bytes; IPC limit is {}",
            encoded.len(),
            crate::ipc::MAX_MESSAGE
        );
    }

    #[test]
    fn corrupt_custom_store_blocks_custom_selection_but_not_builtins() {
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("profiles.json"), b"broken").unwrap();
        let daemon = daemon_with_custom_store(&sysfs, &dir);
        assert!(daemon.status().custom.error.is_some());

        let select = Request {
            id: 90,
            method: "profile.select".into(),
            params: serde_json::json!({"name": "desk"}),
        };
        let response = daemon.handle_request(&select);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "config_corrupt");

        let builtin = Request {
            id: 91,
            method: "profile.select".into(),
            params: serde_json::json!({"name": "quiet"}),
        };
        assert!(daemon.handle_request(&builtin).ok);
        assert_eq!(daemon.status().profile.selected.as_deref(), Some("quiet"));
    }

    #[test]
    fn fixed_preset_applies_through_supervisor_and_records_multi_lease() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 9,
            method: "preset.apply".into(),
            params: serde_json::json!({"name":"performance","confirm":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(response.ok, "{:?}", response.error);
        let result = response.result.expect("result");
        assert_eq!(result["hardware_applied"], true);
        assert_eq!(result["targets"][0]["requested_percent"], 80.0);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 1);
        let status = daemon.status();
        assert_eq!(status.safety.state, SafetyState::Manual);
        assert!(status.safety.manual.is_none());
        assert_eq!(status.safety.preset.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn fixed_preset_applies_to_multiple_supported_fans() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::with_two_cpu_fans());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 12,
            method: "preset.apply".into(),
            params: serde_json::json!({"name":"balanced","confirm":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(response.ok, "{:?}", response.error);
        let result = response.result.expect("result");
        assert_eq!(result["targets"].as_array().unwrap().len(), 2);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            daemon.status().safety.preset.as_ref().map(Vec::len),
            Some(2)
        );
    }
    #[test]
    fn manual_lease_recovers_when_access_control_is_lost() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let nbfc_socket = dir.path().join("nbfc.sock");
        let _listener = UnixListener::bind(&nbfc_socket).unwrap();
        std::fs::set_permissions(&nbfc_socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let config = DaemonConfig {
            nbfc_socket: nbfc_socket.clone(),
            ..DaemonConfig::default()
        };
        let daemon = Daemon::new(backend_trait, SysfsRoot::new(sysfs.path()), config);
        let set = Request {
            id: 17,
            method: "fan.set".into(),
            params: serde_json::json!({"fan":0,"percent":50.0,"confirm":true}),
        };
        let response = daemon.handle_request(&set);
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(daemon.status().safety.state, SafetyState::Manual);

        std::fs::set_permissions(&nbfc_socket, std::fs::Permissions::from_mode(0o666)).unwrap();
        daemon.refresh();
        daemon.tick();
        let status = daemon.status();
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(status.safety.manual.is_none());
        assert!(
            status
                .safety
                .reasons
                .iter()
                .any(|reason| reason.contains("access control"))
        );
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn profile_stops_when_nbfc_access_control_is_lost() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let nbfc_socket = dir.path().join("nbfc-profile.sock");
        let _listener = UnixListener::bind(&nbfc_socket).unwrap();
        std::fs::set_permissions(&nbfc_socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let config = DaemonConfig {
            nbfc_socket: nbfc_socket.clone(),
            ..DaemonConfig::default()
        };
        let daemon = Daemon::new(backend_trait, SysfsRoot::new(sysfs.path()), config);
        let response = daemon.handle_request(&apply_profile_request("quiet", false));
        assert!(response.ok, "{:?}", response.error);
        assert!(daemon.status().profile.active);

        std::fs::set_permissions(&nbfc_socket, std::fs::Permissions::from_mode(0o666)).unwrap();
        daemon.refresh();
        daemon.tick();
        let status = daemon.status();
        assert!(!status.profile.active);
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(
            status
                .safety
                .reasons
                .iter()
                .any(|reason| reason.contains("access control"))
        );
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
    }

    #[test]
    fn protected_operation_revalidates_socket_permissions() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let nbfc_socket = dir.path().join("nbfc.sock");
        let _listener = UnixListener::bind(&nbfc_socket).unwrap();
        std::fs::set_permissions(&nbfc_socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let config = DaemonConfig {
            nbfc_socket: nbfc_socket.clone(),
            ..DaemonConfig::default()
        };
        let daemon = Daemon::new(backend_trait, SysfsRoot::new(sysfs.path()), config);
        assert!(daemon.status().capabilities.unwrap().access_controlled);

        std::fs::set_permissions(&nbfc_socket, std::fs::Permissions::from_mode(0o666)).unwrap();
        let request = Request {
            id: 16,
            method: "preset.apply".into(),
            params: serde_json::json!({"name":"turbo","confirm":true,"dry_run":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(!response.ok);
        let error = response.error.unwrap();
        assert_eq!(error.code, "unsafe_refused");
        assert!(
            error
                .details
                .iter()
                .any(|detail| detail.contains("access-controlled")),
            "{:?}",
            error.details
        );
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn fixed_preset_is_refused_when_nbfc_socket_can_be_bypassed() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let sysfs = sysfs_with_cpu();
        let dir = tempfile::tempdir().unwrap();
        let nbfc_socket = dir.path().join("nbfc.sock");
        let _listener = UnixListener::bind(&nbfc_socket).unwrap();
        std::fs::set_permissions(&nbfc_socket, std::fs::Permissions::from_mode(0o666)).unwrap();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let config = DaemonConfig {
            nbfc_socket,
            ..DaemonConfig::default()
        };
        let daemon = Daemon::new(backend_trait, SysfsRoot::new(sysfs.path()), config);
        let request = Request {
            id: 15,
            method: "preset.apply".into(),
            params: serde_json::json!({"name":"turbo","confirm":true,"dry_run":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(!response.ok);
        let error = response.error.unwrap();
        assert_eq!(error.code, "unsafe_refused");
        assert!(
            error
                .details
                .iter()
                .any(|detail| detail.contains("access-controlled"))
        );
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn fixed_preset_partial_failure_requests_automatic_recovery() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::with_failing_second_fan());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 13,
            method: "preset.apply".into(),
            params: serde_json::json!({"name":"balanced","confirm":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "backend_unavailable");
        // Both fan commands were attempted, but no manual lease was recorded
        // and automatic recovery was requested after the failure.
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 2);
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
        let status = daemon.status();
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(status.safety.preset.is_none());
        assert!(!status.safety.reasons.is_empty());
    }

    #[test]
    fn fixed_preset_unconfirmed_target_requests_recovery() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::with_ignored_target());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 14,
            method: "preset.apply".into(),
            params: serde_json::json!({"name":"turbo","confirm":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(response.error.unwrap().code, "verification_failed");
        assert!(backend.auto_calls.load(Ordering::Relaxed) >= 1);
        let status = daemon.status();
        assert_eq!(status.safety.state, SafetyState::FailSafe);
        assert!(status.safety.preset.is_none());
    }

    #[test]
    fn fixed_preset_is_blocked_without_protected_manual_control() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::with_version(0, 5, 3));
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 10,
            method: "preset.apply".into(),
            params: serde_json::json!({"name":"turbo","confirm":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn fixed_preset_dry_run_is_preview_only() {
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let daemon = Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 11,
            method: "preset.apply".into(),
            params: serde_json::json!({"name":"silent","confirm":true,"dry_run":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn policy_gate_is_enforced_through_daemon_state() {
        let sysfs = sysfs_with_cpu();
        let daemon = Daemon::new(
            Arc::new(FakeBackend::new()),
            SysfsRoot::new(sysfs.path()),
            DaemonConfig::default(),
        );
        let request = Request {
            id: 1,
            method: "fan.set".into(),
            params: serde_json::json!({"fan":0,"percent":50.0,"confirm":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(response.ok, "{:?}", response.error);
        let result = response.result.expect("result");
        assert_eq!(result["verified"], true);
        assert_eq!(result["safety"]["state"], "manual");
    }

    #[derive(Default)]
    struct FakeNotifier {
        events: Mutex<Vec<String>>,
        interval: Option<Duration>,
    }

    impl crate::systemd::Notifier for FakeNotifier {
        fn ready(&self) -> Result<(), String> {
            self.events.lock().unwrap().push("ready".into());
            Ok(())
        }
        fn watchdog(&self) -> Result<(), String> {
            self.events.lock().unwrap().push("watchdog".into());
            Ok(())
        }
        fn stopping(&self) -> Result<(), String> {
            self.events.lock().unwrap().push("stopping".into());
            Ok(())
        }
        fn watchdog_interval(&self) -> Option<Duration> {
            self.interval
        }
    }

    #[test]
    fn existing_socket_parent_permissions_are_not_changed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare_runtime_dir_with_group(parent, Some("root")).unwrap();
        let mode = std::fs::metadata(parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    fn wait_for_notifier_event(notifier: &FakeNotifier, event: &str, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if notifier.events.lock().unwrap().iter().any(|e| e == event) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn daemon_reports_ready_and_stopping_with_socket_mode() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("neosensed.sock");
        let config = DaemonConfig {
            socket: socket.clone(),
            interval_ms: 500,
            custom_profiles_path: dir.path().join("profiles.json"),
            startup_safe_auto: false,
            ..DaemonConfig::default()
        };
        let daemon = Arc::new(Daemon::new(
            Arc::new(FakeBackend::new()),
            SysfsRoot::new("/nonexistent-neosense-test"),
            config,
        ));
        let notifier = Arc::new(FakeNotifier {
            events: Mutex::new(Vec::new()),
            interval: Some(Duration::from_secs(3)),
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let runner = Arc::clone(&daemon);
        let stop = Arc::clone(&shutdown);
        let recorder = Arc::clone(&notifier);
        let handle = thread::spawn(move || runner.run_with_notifier(stop, recorder));
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let metadata = std::fs::metadata(&socket).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o660);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(metadata.gid(), unsafe { libc::getegid() });
        assert!(
            wait_for_notifier_event(&notifier, "ready", Duration::from_secs(10)),
            "supervisor did not report READY"
        );
        assert!(
            wait_for_notifier_event(&notifier, "watchdog", Duration::from_secs(10)),
            "supervisor did not emit a healthy watchdog"
        );
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
        assert!(
            wait_for_notifier_event(&notifier, "stopping", Duration::from_secs(10)),
            "supervisor did not report STOPPING"
        );
    }

    #[test]
    fn fail_safe_is_idempotent_and_reports_failure() {
        let backend = FakeBackend::new();
        assert!(fail_safe(&backend).is_ok());
        assert!(fail_safe(&backend).is_ok());
        let failing = FakeBackend::with_auto_failure();
        assert!(fail_safe(&failing).is_err());
    }

    #[test]
    fn startup_failure_does_not_emit_ready() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("occupied.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        let config = DaemonConfig {
            socket: socket.clone(),
            interval_ms: 500,
            startup_safe_auto: false,
            ..DaemonConfig::default()
        };
        let daemon = Arc::new(Daemon::new(
            Arc::new(FakeBackend::new()),
            SysfsRoot::new("/nonexistent-neosense-test"),
            config,
        ));
        let notifier = Arc::new(FakeNotifier {
            events: Mutex::new(Vec::new()),
            interval: Some(Duration::from_secs(3)),
        });
        let error = daemon
            .run_with_notifier(Arc::new(AtomicBool::new(false)), notifier.clone())
            .unwrap_err();
        assert!(error.contains("already listening"));
        assert!(!notifier.events.lock().unwrap().iter().any(|e| e == "ready"));
    }

    #[test]
    fn watchdog_stops_when_health_fails() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("neosensed.sock");
        let config = DaemonConfig {
            socket: socket.clone(),
            interval_ms: 60_000,
            startup_safe_auto: false,
            ..DaemonConfig::default()
        };
        let daemon = Arc::new(Daemon::new(
            Arc::new(FakeBackend::new()),
            SysfsRoot::new("/nonexistent-neosense-test"),
            config,
        ));
        let notifier = Arc::new(FakeNotifier {
            events: Mutex::new(Vec::new()),
            interval: Some(Duration::from_secs(3)),
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let runner = Arc::clone(&daemon);
        let stop = Arc::clone(&shutdown);
        let recorder = Arc::clone(&notifier);
        let handle = thread::spawn(move || runner.run_with_notifier(stop, recorder));
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            wait_for_notifier_event(&notifier, "watchdog", Duration::from_secs(10)),
            "a healthy watchdog must be emitted"
        );
        let before = notifier
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| *e == "watchdog")
            .count();
        daemon.state.lock().unwrap().sensors.collected_at_ms = 0;
        thread::sleep(Duration::from_secs(2));
        let after = notifier
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| *e == "watchdog")
            .count();
        assert_eq!(before, after, "the watchdog must stop when health is false");
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn system_group_lookup_is_available_for_socket_ownership() {
        let gid = group_gid("root").unwrap();
        assert_eq!(gid, 0);
    }

    #[test]
    fn health_detects_stale_sampler_and_inconsistent_manual_state() {
        let daemon = Daemon::new(
            Arc::new(FakeBackend::new()),
            SysfsRoot::new("/nonexistent-neosense-test"),
            DaemonConfig::default(),
        );
        assert!(daemon.health().healthy);
        daemon.state.lock().unwrap().sensors.collected_at_ms = 0;
        let health = daemon.health();
        assert!(!health.healthy);
        assert!(health.reason.unwrap().contains("too old"));

        let daemon = Daemon::new(
            Arc::new(FakeBackend::new()),
            SysfsRoot::new("/nonexistent-neosense-test"),
            DaemonConfig::default(),
        );
        {
            let mut state = daemon.state.lock().unwrap();
            state.safety.state = SafetyState::Manual;
            state.safety.manual = None;
        }
        assert!(!daemon.health().healthy);
    }

    #[test]
    fn daemon_serves_status_over_a_socket_without_hardware() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("neosensed.sock");
        let sysfs = sysfs_with_cpu();
        let backend = Arc::new(FakeBackend::new());
        let backend_trait: Arc<dyn FanBackend> = backend.clone();
        let config = DaemonConfig {
            socket: socket.clone(),
            interval_ms: 500,
            custom_profiles_path: dir.path().join("profiles.json"),
            startup_safe_auto: false,
            ..DaemonConfig::default()
        };
        let daemon = Arc::new(Daemon::new(
            backend_trait,
            SysfsRoot::new(sysfs.path()),
            config,
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let runner = Arc::clone(&daemon);
        let stop = Arc::clone(&shutdown);
        let handle = thread::spawn(move || runner.run(stop));

        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let mut client = crate::ipc::Client::connect(&socket).unwrap();
        let value = client.call("status.get", serde_json::json!({})).unwrap();
        assert!(value["sensors"].is_object());
        assert_eq!(value["nbfc"]["fans"].as_array().unwrap().len(), 1);
        let mut profile_client = crate::ipc::Client::connect(&socket).unwrap();
        let selected = profile_client
            .call("profile.select", serde_json::json!({"name":"quiet"}))
            .unwrap();
        assert_eq!(selected["hardware_applied"], false);
        assert_eq!(selected["profile"]["mode"], "simulated");
        assert_eq!(
            selected["profile"]["targets"][0]["target_percent"],
            serde_json::json!(30.0)
        );
        let mut get_client = crate::ipc::Client::connect(&socket).unwrap();
        let current = get_client
            .call("profile.get", serde_json::json!({}))
            .unwrap();
        assert_eq!(current["profile"]["selected"], "quiet");
        let mut custom_client = crate::ipc::Client::connect(&socket).unwrap();
        custom_client
            .call(
                "custom.create",
                serde_json::json!({"profile": custom_profile_value("socket-curve")}),
            )
            .unwrap();
        let mut custom_select_client = crate::ipc::Client::connect(&socket).unwrap();
        let custom_selected = custom_select_client
            .call("profile.select", serde_json::json!({"name":"socket-curve"}))
            .unwrap();
        assert_eq!(custom_selected["hardware_applied"], false);
        assert_eq!(custom_selected["profile"]["selected"], "socket-curve");
        assert!(custom_selected["profile"]["targets"][0]["target_percent"].is_number());
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 0);
        let mut preset_client = crate::ipc::Client::connect(&socket).unwrap();
        let preset = preset_client
            .call(
                "preset.apply",
                serde_json::json!({"name":"performance","confirm":true}),
            )
            .unwrap();
        assert_eq!(preset["hardware_applied"], true);
        assert_eq!(preset["targets"][0]["requested_percent"], 80.0);
        assert_eq!(backend.set_speed_calls.load(Ordering::Relaxed), 1);

        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn manual_is_refused_on_nbfc_053_even_with_confirmation() {
        let daemon = Daemon::new(
            Arc::new(FakeBackend::with_version(0, 5, 3)),
            SysfsRoot::new("/nonexistent-neosense-test"),
            DaemonConfig::default(),
        );
        daemon.state.lock().unwrap().sensors = snapshot();
        let request = Request {
            id: 1,
            method: "fan.set".into(),
            params: serde_json::json!({"fan":0,"percent":50.0,"confirm":true}),
        };
        let response = daemon.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("unsafe_refused")
        );
    }

    #[test]
    fn manual_without_confirmation_is_refused() {
        let daemon = Daemon::new(
            Arc::new(FakeBackend::new()),
            SysfsRoot::new("/nonexistent-neosense-test"),
            DaemonConfig::default(),
        );
        daemon.state.lock().unwrap().sensors = snapshot();
        let request = Request {
            id: 1,
            method: "fan.set".into(),
            params: serde_json::json!({"fan":0,"percent":50.0}),
        };
        let response = daemon.handle_request(&request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("confirmation_required")
        );
    }
}
