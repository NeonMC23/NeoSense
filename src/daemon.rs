use crate::custom_profiles::{CustomProfile, CustomStatus, Store as CustomStore, StoreError};
use crate::ipc::{ApiError, Request, Response, read_message, write_message};
use crate::model::{SensorSnapshot, now_ms};
use crate::nbfc::{FanBackend, NbfcCapabilities, NbfcSensorConfig, NbfcStatus, NbfcVersion};
use crate::policy::{
    FanRole, ManualLease, PolicyDecision, SafetyConfig, SafetyReport, SafetyState,
    assess_manual_request, fan_role,
};
use crate::regulation::{
    FanObservation, ProfileState, RegulationMode, evaluate_profile, mapping_matches,
};
use crate::sensors::{SysfsRoot, collect_all};
use crate::service::{ServiceStatus, nbfc_service_status};
use crate::systemd::{Notifier, SystemdNotifier};
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub socket: PathBuf,
    pub interval_ms: u64,
    pub safety: SafetyConfig,
    /// Persistent user curve store owned (and writable) by the supervisor.
    pub custom_profiles_path: PathBuf,
    /// If NBFC starts in a fixed state, return it to auto at supervisor start.
    pub startup_safe_auto: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket: PathBuf::from("/run/neosense/neosensed.sock"),
            interval_ms: 2_000,
            safety: SafetyConfig::default(),
            custom_profiles_path: crate::custom_profiles::system_path(),
            startup_safe_auto: true,
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

struct State {
    nbfc_version: Option<NbfcVersion>,
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
}

impl Daemon {
    pub fn new(backend: Arc<dyn FanBackend>, root: SysfsRoot, config: DaemonConfig) -> Self {
        let custom = CustomStore::new(config.custom_profiles_path.clone()).status();
        let daemon = Self {
            backend,
            root,
            config,
            state: Mutex::new(State {
                nbfc_version: None,
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
        let now = now_ms();
        let (
            cached_version,
            cached_config,
            config_age,
            old_selected,
            selected_profile,
            old_profile,
            custom,
        ) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.nbfc_version,
                state.sensor_config.clone(),
                now.saturating_sub(state.sensor_config_refreshed_at_ms),
                state
                    .nbfc
                    .as_ref()
                    .and_then(|status| status.selected_config.clone()),
                state.profile.selected.clone(),
                state.profile.clone(),
                state.custom.clone(),
            )
        };

        // The NBFC version is immutable for the lifetime of a process; do not
        // spawn `nbfc --version` every sampling interval. Sensor mappings are
        // refreshed less often but immediately when the selected config changes.
        let version = cached_version.or_else(|| self.backend.version().ok());
        let capabilities = version.map(NbfcCapabilities::from_version);
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
        let service = nbfc_service_status();
        let profile = evaluate_selected_profile(
            selected_profile.as_deref(),
            &sensors,
            nbfc.as_ref(),
            sensor_config.as_ref(),
            &old_profile,
            &custom.profiles,
            custom.error.as_deref(),
            now,
        );
        let mut state = self.state.lock().expect("poisoned mutex");
        state.sensors = sensors;
        state.service = service;
        state.nbfc_version = version;
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
            "status.get" => Response::ok(request.id, self.status()),
            "profile.get" => Response::ok(
                request.id,
                serde_json::json!({"schema":"neosense.profile_state/v1","profile": self.status().profile}),
            ),
            "profile.select" => self.select_profile(request),
            "profile.clear" => {
                self.state.lock().expect("poisoned mutex").profile = ProfileState::default();
                self.refresh();
                Response::ok(
                    request.id,
                    serde_json::json!({
                        "schema": "neosense.profile_state/v1",
                        "profile": self.status().profile,
                        "hardware_applied": false
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
                match serde_json::from_value::<CustomProfileBody>(request.params.clone()) {
                    Ok(body) => {
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
            "custom.delete" => match serde_json::from_value::<CustomDelete>(request.params.clone())
            {
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
            },
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
            "custom.rename" => match serde_json::from_value::<CustomRename>(request.params.clone())
            {
                Ok(body) => {
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
            },
            "custom.reset" => match serde_json::from_value::<CustomReset>(request.params.clone()) {
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
                        match CustomStore::new(self.config.custom_profiles_path.clone()).reset(true)
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
            },
            "fan.auto" => match self.auto_internal("user request") {
                Ok(result) => Response::ok(request.id, result),
                Err(e) => Response::error(request.id, e),
            },
            "fan.set" => match serde_json::from_value::<ManualSet>(request.params.clone()) {
                Ok(set) => match self.manual_internal(set) {
                    Ok(result) => Response::ok(request.id, result),
                    Err(e) => Response::error(request.id, e),
                },
                Err(e) => Response::error(
                    request.id,
                    ApiError::new("bad_request", format!("invalid fan.set parameters: {e}")),
                ),
            },
            "preset.apply" => match serde_json::from_value::<PresetSet>(request.params.clone()) {
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
            },
            _ => Response::error(
                request.id,
                ApiError::new(
                    "unknown_method",
                    format!("unknown IPC method: {}", request.method),
                ),
            ),
        }
    }

    fn select_profile(&self, request: &Request) -> Response {
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
        let (snapshot, status, capabilities, sensor_config, safety_config) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.sensors.clone(),
                state.nbfc.clone(),
                state.capabilities.clone(),
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
        for (index, name, target) in &plans {
            let output = match self.backend.set_speed(*index, *target) {
                Ok(output) => output,
                Err(error) => {
                    self.recover_after_preset_failure(&format!(
                        "fan {index} command failed: {error}"
                    ));
                    return Err(ApiError::new("backend_unavailable", error));
                }
            };
            if !output.success {
                self.recover_after_preset_failure(&format!("fan {index} command failed"));
                return Err(ApiError::new("backend_failed", output.combined_error()));
            }
            let _ = name;
        }
        self.refresh();
        let post = self.state.lock().expect("poisoned mutex").nbfc.clone();
        let Some(post_status) = post.as_ref() else {
            self.recover_after_preset_failure("NBFC status unavailable after preset writes");
            return Err(ApiError::new(
                "verification_failed",
                "NBFC status unavailable after preset; automatic recovery requested",
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
                self.recover_after_preset_failure("preset target or manual mode was not confirmed");
                return Err(ApiError::new(
                    "verification_failed",
                    "preset target not confirmed; automatic recovery requested",
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
            "message": "preset targets confirmed by backend; EC readback is not RPM"
        }))
    }

    fn recover_after_preset_failure(&self, reason: &str) {
        let _ = self.backend.set_auto();
        self.refresh();
        let mut state = self.state.lock().expect("poisoned mutex");
        state.safety.state = SafetyState::FailSafe;
        state.safety.manual = None;
        state.safety.preset = None;
        state.safety.reasons = vec![reason.to_string()];
        state.safety.updated_at_ms = now_ms();
    }

    fn auto_internal(&self, reason: &str) -> Result<ControlResult, ApiError> {
        let output = self
            .backend
            .set_auto()
            .map_err(|e| ApiError::new("backend_unavailable", e))?;
        if !output.success {
            return Err(ApiError::new("backend_failed", output.combined_error()));
        }
        self.refresh();
        let status = self.state.lock().expect("poisoned mutex").nbfc.clone();
        let verified = status.as_ref().map(NbfcStatus::all_auto).unwrap_or(false);
        if !verified {
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
            message: "NBFC automatic mode confirmed".to_string(),
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

        let (snapshot, status, capabilities, sensor_config, safety_config) = {
            let state = self.state.lock().expect("poisoned mutex");
            (
                state.sensors.clone(),
                state.nbfc.clone(),
                state.capabilities.clone(),
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
            let _ = self.backend.set_auto();
            self.refresh();
            self.set_safety(SafetyReport {
                state: SafetyState::FailSafe,
                reasons: vec!["NBFC status unavailable after write".to_string()],
                manual: None,
                preset: None,
                updated_at_ms: now_ms(),
            });
            return Err(ApiError::new(
                "verification_failed",
                "NBFC status unavailable after write; automatic recovery requested",
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
            let _ = self.backend.set_auto();
            self.refresh();
            self.set_safety(SafetyReport {
                state: SafetyState::FailSafe,
                reasons: post_decision.reasons,
                manual: None,
                preset: None,
                updated_at_ms: now_ms(),
            });
            return Err(ApiError::new(
                "verification_failed",
                "sensor or source became invalid after write; automatic recovery requested",
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
            let _ = self.backend.set_auto();
            self.refresh();
            let safety = SafetyReport {
                state: SafetyState::FailSafe,
                reasons: vec!["target or manual mode was not confirmed after write".to_string()],
                manual: None,
                preset: None,
                updated_at_ms: now_ms(),
            };
            self.set_safety(safety);
            return Err(ApiError::new(
                "verification_failed",
                "speed not confirmed; automatic recovery requested",
            ));
        }

        let safety = SafetyReport {
            state: SafetyState::Manual,
            reasons: Vec::new(),
            manual: Some(ManualLease {
                fan_index: request.fan,
                percent: request.percent,
                expires_at_ms: now_ms() + safety_config.manual_ttl_ms,
            }),
            preset: None,
            updated_at_ms: now_ms(),
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
            message: "target and manual mode confirmed by NBFC".to_string(),
            safety,
        })
    }

    fn set_safety(&self, safety: SafetyReport) {
        self.state.lock().expect("poisoned mutex").safety = safety;
    }

    /// Periodic safety enforcement. Called by the sampler thread.
    pub fn tick(&self) {
        let (leases, critical, cpu_ok, gpu_ok) = {
            let state = self.state.lock().expect("poisoned mutex");
            let now = now_ms();
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
            (leases, critical, cpu_ok, gpu_ok)
        };

        if !leases.is_empty() {
            let expired = leases.iter().any(|lease| now_ms() >= lease.expires_at_ms);
            if expired || critical || !cpu_ok || !gpu_ok {
                let reason = if expired {
                    "manual lease expired"
                } else if critical {
                    "NBFC critical mode"
                } else {
                    "critical sensor missing or stale"
                };
                match self.auto_internal(reason) {
                    Ok(_) => {
                        let mut state = self.state.lock().expect("poisoned mutex");
                        if !expired && (critical || !cpu_ok || !gpu_ok) {
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

        while !shutdown.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let daemon = Arc::clone(self);
                    thread::spawn(move || handle_connection(daemon, stream));
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
    let builtin = crate::profiles::find(name);
    let custom = customs.iter().find(|profile| profile.name == name);
    let (curve, roles) = if let Some(profile) = builtin {
        (profile.curve, None)
    } else if let Some(profile) = custom {
        (Some(profile.curve_spec()), Some(profile.fan_roles.clone()))
    } else if let Some(error) = custom_error {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(RegulationMode::Simulated),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some(format!("custom profile store unavailable: {error}")),
        };
    } else {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(RegulationMode::Simulated),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some(format!("unknown profile: {name}")),
        };
    };
    let Some(curve) = curve else {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(RegulationMode::Simulated),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some(format!("profile {name} has no regulation curve")),
        };
    };
    let Some(status) = nbfc else {
        return ProfileState {
            selected: Some(name.to_string()),
            mode: Some(RegulationMode::Simulated),
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
            let mapping_ok = source
                .map(|source| mapping_matches(role, &source.sensors))
                .unwrap_or(false);
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
            mode: Some(RegulationMode::Simulated),
            evaluated_at_ms: now,
            targets: Vec::new(),
            reason: Some("no backend fan matches the profile's selected fan roles".to_string()),
        };
    }
    match evaluate_profile(&curve, snapshot, &fans, RegulationMode::Simulated, now) {
        Ok(targets) => ProfileState {
            selected: Some(name.to_string()),
            mode: Some(RegulationMode::Simulated),
            evaluated_at_ms: now,
            targets,
            reason: Some(if custom.is_some() {
                "user curve simulation only; no fan write performed".to_string()
            } else {
                "simulation only; no fan write performed".to_string()
            }),
        },
        Err(error) => ProfileState {
            selected: Some(name.to_string()),
            mode: Some(RegulationMode::Simulated),
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
    use std::sync::atomic::AtomicUsize;

    struct FakeBackend {
        fixed: Mutex<bool>,
        target: Mutex<f64>,
        version: NbfcVersion,
        fail_auto: bool,
        version_calls: AtomicUsize,
        config_calls: AtomicUsize,
        set_speed_calls: AtomicUsize,
        auto_calls: AtomicUsize,
        fail_speed_for: Mutex<Option<u32>>,
        ignore_speed: bool,
        two_cpu_fans: bool,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                fixed: Mutex::new(false),
                target: Mutex::new(35.0),
                version: NbfcVersion {
                    major: 0,
                    minor: 5,
                    patch: 4,
                },
                fail_auto: false,
                version_calls: AtomicUsize::new(0),
                config_calls: AtomicUsize::new(0),
                set_speed_calls: AtomicUsize::new(0),
                auto_calls: AtomicUsize::new(0),
                fail_speed_for: Mutex::new(None),
                ignore_speed: false,
                two_cpu_fans: false,
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
            let mut backend = Self::new();
            backend.version = NbfcVersion {
                major,
                minor,
                patch,
            };
            backend
        }
    }

    impl FanBackend for FakeBackend {
        fn version(&self) -> Result<NbfcVersion, String> {
            self.version_calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.version)
        }
        fn status(&self, _version: NbfcVersion) -> Result<NbfcStatus, String> {
            let fixed = *self.fixed.lock().unwrap();
            let target = *self.target.lock().unwrap();
            let mut fans = vec![NbfcFan {
                index: 0,
                name: "CPU fan".into(),
                temperature: Some(55.0),
                critical_temperature: Some(95.0),
                auto_mode: Some(!fixed),
                critical: Some(false),
                current_speed: Some(if fixed { target } else { 35.0 }),
                target_speed: Some(if fixed { target } else { 35.0 }),
                requested_speed: Some(if fixed { target } else { -1.0 }),
                speed_steps: Some(100),
            }];
            if self.two_cpu_fans {
                fans.push(NbfcFan {
                    index: 1,
                    name: "CPU fan 2".into(),
                    temperature: Some(55.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(!fixed),
                    critical: Some(false),
                    current_speed: Some(if fixed { target } else { 35.0 }),
                    target_speed: Some(if fixed { target } else { 35.0 }),
                    requested_speed: Some(if fixed { target } else { -1.0 }),
                    speed_steps: Some(100),
                });
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
            self.auto_calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_auto {
                return Err("backend auto failure".into());
            }
            *self.fixed.lock().unwrap() = false;
            Ok(ok_output())
        }
        fn set_speed(&self, fan: u32, percent: f64) -> Result<crate::util::CommandOutput, String> {
            self.set_speed_calls.fetch_add(1, Ordering::Relaxed);
            if *self.fail_speed_for.lock().unwrap() == Some(fan) {
                return Err(format!("fake backend rejected fan {fan}"));
            }
            if !self.ignore_speed {
                *self.fixed.lock().unwrap() = true;
                *self.target.lock().unwrap() = percent;
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
                    name: "CPU fan 2".into(),
                    sensors: vec!["@CPU".into()],
                    algorithm: Some("Average".into()),
                });
            }
            Ok(NbfcSensorConfig { fans })
        }
        fn list_configs(&self) -> Result<Vec<String>, String> {
            Ok(vec!["test".into()])
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
        thread::sleep(Duration::from_millis(1_200));
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap().unwrap();
        let events = notifier.events.lock().unwrap().clone();
        assert!(events.iter().any(|e| e == "ready"));
        assert!(events.iter().any(|e| e == "watchdog"));
        assert!(events.iter().any(|e| e == "stopping"));
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
        thread::sleep(Duration::from_millis(1_200));
        let before = notifier
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| *e == "watchdog")
            .count();
        assert!(before >= 1, "a healthy watchdog must be emitted");
        daemon.state.lock().unwrap().sensors.collected_at_ms = 0;
        thread::sleep(Duration::from_millis(1_200));
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
