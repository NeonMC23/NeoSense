use crate::ipc::{ApiError, Request, Response, read_message, write_message};
use crate::model::{SensorSnapshot, now_ms};
use crate::nbfc::{FanBackend, NbfcCapabilities, NbfcSensorConfig, NbfcStatus, NbfcVersion};
use crate::policy::{
    FanRole, ManualLease, PolicyDecision, SafetyConfig, SafetyReport, SafetyState,
    assess_manual_request, fan_role,
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
    /// If NBFC starts in a fixed state, return it to auto at supervisor start.
    pub startup_safe_auto: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket: PathBuf::from("/run/neosense/neosensed.sock"),
            interval_ms: 2_000,
            safety: SafetyConfig::default(),
            startup_safe_auto: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub generated_at_ms: u64,
    pub nbfc_version: Option<String>,
    pub capabilities: Option<NbfcCapabilities>,
    pub nbfc: Option<NbfcStatus>,
    pub service: ServiceStatus,
    pub sensor_config: Option<NbfcSensorConfig>,
    pub sensors: SensorSnapshot,
    pub safety: SafetyReport,
    pub health: DaemonHealth,
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

struct State {
    nbfc_version: Option<NbfcVersion>,
    capabilities: Option<NbfcCapabilities>,
    nbfc: Option<NbfcStatus>,
    service: ServiceStatus,
    sensor_config: Option<NbfcSensorConfig>,
    sensors: SensorSnapshot,
    safety: SafetyReport,
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
                sensors: SensorSnapshot::empty(),
                safety: SafetyReport::safe_auto(),
                last_error: None,
            }),
        };
        daemon.refresh();
        daemon
    }

    pub fn refresh(&self) {
        let sensors = collect_all(&self.root);
        let service = nbfc_service_status();
        let version = self.backend.version().ok();
        let capabilities = version.map(NbfcCapabilities::from_version);
        let nbfc = match (version, &capabilities) {
            (Some(v), Some(_)) => self.backend.status(v).ok(),
            _ => None,
        };
        let sensor_config = self.backend.sensors_show().ok();
        let error = if version.is_none() {
            Some("NBFC indisponible".to_string())
        } else if nbfc.is_none() {
            Some("statut NBFC indisponible".to_string())
        } else {
            None
        };

        let mut state = self.state.lock().expect("mutex empoisonné");
        state.sensors = sensors;
        state.service = service;
        state.nbfc_version = version;
        state.capabilities = capabilities;
        state.nbfc = nbfc;
        state.sensor_config = sensor_config;
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
            .expect("mutex empoisonné")
            .nbfc
            .as_ref()
            .map(|s| s.fans.iter().any(|f| f.auto_mode == Some(false)))
            .unwrap_or(false);
        if fixed {
            self.auto_internal("état fixe détecté au démarrage")
                .map(|_| ())
                .map_err(|e| e.message)
        } else {
            Ok(())
        }
    }

    pub fn status(&self) -> DaemonStatus {
        let state = self.state.lock().expect("mutex empoisonné");
        DaemonStatus {
            generated_at_ms: now_ms(),
            nbfc_version: state.nbfc_version.map(|v| v.to_string()),
            capabilities: state.capabilities.clone(),
            nbfc: state.nbfc.clone(),
            service: state.service.clone(),
            sensor_config: state.sensor_config.clone(),
            sensors: state.sensors.clone(),
            safety: state.safety.clone(),
            health: health_from_state(&state, &self.config),
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
                reason: Some("état interne empoisonné".to_string()),
            },
        }
    }

    pub fn handle_request(&self, request: &Request) -> Response {
        match request.method.as_str() {
            "ping" => Response::ok(request.id, serde_json::json!({"pong": true})),
            "status.get" => Response::ok(request.id, self.status()),
            "fan.auto" => match self.auto_internal("demande utilisateur") {
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
                    ApiError::new("bad_request", format!("paramètres fan.set invalides: {e}")),
                ),
            },
            _ => Response::error(
                request.id,
                ApiError::new(
                    "unknown_method",
                    format!("méthode IPC inconnue: {}", request.method),
                ),
            ),
        }
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
        let status = self.state.lock().expect("mutex empoisonné").nbfc.clone();
        let verified = status.as_ref().map(NbfcStatus::all_auto).unwrap_or(false);
        if !verified {
            let safety = SafetyReport {
                state: SafetyState::FailSafe,
                reasons: vec![format!("retour auto non confirmé après: {reason}")],
                manual: None,
                updated_at_ms: now_ms(),
            };
            self.set_safety(safety);
            return Err(ApiError::new(
                "verification_failed",
                "commande auto acceptée mais mode automatique non confirmé",
            ));
        }

        let safety = SafetyReport::safe_auto();
        self.set_safety(safety.clone());
        Ok(ControlResult {
            action: "auto".to_string(),
            fan_index: None,
            requested_percent: None,
            target_percent: None,
            reported_percent: None,
            readback_plausible: None,
            verified,
            dry_run: false,
            message: "mode automatique NBFC confirmé".to_string(),
            safety,
        })
    }

    fn manual_internal(&self, request: ManualSet) -> Result<ControlResult, ApiError> {
        if !request.confirm {
            return Err(ApiError::new(
                "confirmation_required",
                "le contrôle manuel exige une confirmation explicite",
            ));
        }

        let (snapshot, status, capabilities, sensor_config, safety_config) = {
            let state = self.state.lock().expect("mutex empoisonné");
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
                "statut NBFC indisponible; contrôle manuel refusé",
            ));
        };
        let Some(capabilities) = capabilities else {
            return Err(ApiError::new(
                "backend_unavailable",
                "capacités NBFC indisponibles; contrôle manuel refusé",
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
                "contrôle manuel refusé par la politique de sécurité",
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
                message: "demande valide; aucune écriture effectuée (dry-run)".to_string(),
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
            let state = self.state.lock().expect("mutex empoisonné");
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
                reasons: vec!["statut NBFC indisponible après écriture".to_string()],
                manual: None,
                updated_at_ms: now_ms(),
            });
            return Err(ApiError::new(
                "verification_failed",
                "statut NBFC indisponible après écriture; retour automatique demandé",
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
                updated_at_ms: now_ms(),
            });
            return Err(ApiError::new(
                "verification_failed",
                "capteur ou source devenu invalide après écriture; retour automatique demandé",
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
                reasons: vec![
                    "la cible ou le mode manuel n'a pas été confirmé après écriture".to_string(),
                ],
                manual: None,
                updated_at_ms: now_ms(),
            };
            self.set_safety(safety);
            return Err(ApiError::new(
                "verification_failed",
                "vitesse non confirmée; retour automatique demandé",
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
            message: "cible et mode manuel confirmés par NBFC".to_string(),
            safety,
        })
    }

    fn set_safety(&self, safety: SafetyReport) {
        self.state.lock().expect("mutex empoisonné").safety = safety;
    }

    /// Periodic safety enforcement. Called by the sampler thread.
    pub fn tick(&self) {
        let (manual, critical, cpu_ok, gpu_ok) = {
            let state = self.state.lock().expect("mutex empoisonné");
            let now = now_ms();
            let manual = state.safety.manual.clone();
            let critical = state
                .nbfc
                .as_ref()
                .map(NbfcStatus::any_critical)
                .unwrap_or(false);
            let cpu_ok = state
                .sensors
                .cpu_package(now, self.config.safety.stale_after_ms)
                .is_some();
            let needs_gpu = manual.as_ref().is_some_and(|lease| {
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
            (manual, critical, cpu_ok, gpu_ok)
        };

        if let Some(lease) = manual {
            let expired = now_ms() >= lease.expires_at_ms;
            if expired || critical || !cpu_ok || !gpu_ok {
                let reason = if expired {
                    "lease manuel expiré"
                } else if critical {
                    "NBFC en mode critique"
                } else {
                    "capteur critique absent ou périmé"
                };
                match self.auto_internal(reason) {
                    Ok(_) => {
                        let mut state = self.state.lock().expect("mutex empoisonné");
                        if !expired && (critical || !cpu_ok || !gpu_ok) {
                            state.safety.state = SafetyState::Degraded;
                            state.safety.reasons = vec![reason.to_string()];
                        }
                    }
                    Err(error) => {
                        let mut state = self.state.lock().expect("mutex empoisonné");
                        state.safety.state = SafetyState::FailSafe;
                        state.safety.manual = None;
                        state.safety.reasons = vec![
                            reason.to_string(),
                            format!("échec du retour auto: {}", error.message),
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
                    "un superviseur écoute déjà sur {}",
                    socket.display()
                ));
            }
            std::fs::remove_file(socket)
                .map_err(|e| format!("suppression socket obsolète: {e}"))?;
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
            return Err(format!("socket non bloquant: {error}"));
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
                        eprintln!("neosensed: WATCHDOG=1 impossible: {error}");
                    }
                } else {
                    eprintln!(
                        "neosensed: watchdog suspendu: {}",
                        health.reason.as_deref().unwrap_or("santé inconnue")
                    );
                }
            }
        }

        if let Err(error) = notifier.stopping() {
            eprintln!("neosensed: STOPPING=1 impossible: {error}");
        }
        let _ = sampler_thread.join();
        let _ = self.auto_internal("arrêt du superviseur");
        let _ = std::fs::remove_file(socket);
        match run_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn health_from_state(state: &State, config: &DaemonConfig) -> DaemonHealth {
    let now = now_ms();
    let max_age = config.interval_ms.saturating_mul(3).max(5_000);
    let age = now.saturating_sub(state.sensors.collected_at_ms);
    if age > max_age {
        return DaemonHealth {
            healthy: false,
            reason: Some(format!("échantillon capteurs trop ancien ({age} ms)")),
        };
    }
    if state.safety.state == SafetyState::Manual && state.safety.manual.is_none() {
        return DaemonHealth {
            healthy: false,
            reason: Some("état manuel sans bail cohérent".to_string()),
        };
    }
    if state.safety.state == SafetyState::Manual && state.nbfc.is_none() {
        return DaemonHealth {
            healthy: false,
            reason: Some("NBFC indisponible pendant un contrôle manuel".to_string()),
        };
    }
    DaemonHealth {
        healthy: true,
        reason: None,
    }
}

fn prepare_runtime_dir(parent: &Path) -> Result<(), String> {
    if !parent.exists() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    if let Some(group) = socket_group() {
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o750))
            .map_err(|e| format!("chmod {}: {e}", parent.display()))?;
        chown_path(parent, 0, group_gid(&group)?)?;
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
    let name_c = CString::new(name).map_err(|_| "nom de groupe invalide".to_string())?;
    let group = unsafe { libc::getgrnam(name_c.as_ptr()) };
    if group.is_null() {
        return Err(format!("groupe système introuvable: {name}"));
    }
    let gid = unsafe { (*group).gr_gid };
    Ok(gid as u32)
}

fn chown_path(path: &Path, uid: u32, gid: u32) -> Result<(), String> {
    let path_c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("chemin invalide: {}", path.display()))?;
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
        Ok("mode automatique NBFC confirmé".to_string())
    } else {
        Err("mode automatique NBFC non confirmé".to_string())
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

    struct FakeBackend {
        fixed: Mutex<bool>,
        target: Mutex<f64>,
        version: NbfcVersion,
        fail_auto: bool,
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
            }
        }

        fn with_auto_failure() -> Self {
            let mut backend = Self::new();
            backend.fail_auto = true;
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
            Ok(self.version)
        }
        fn status(&self, _version: NbfcVersion) -> Result<NbfcStatus, String> {
            let fixed = *self.fixed.lock().unwrap();
            Ok(NbfcStatus {
                version: None,
                read_only: Some(false),
                selected_config: Some("test".into()),
                fans: vec![NbfcFan {
                    index: 0,
                    name: "CPU fan".into(),
                    temperature: Some(55.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(!fixed),
                    critical: Some(false),
                    current_speed: Some(if fixed {
                        *self.target.lock().unwrap()
                    } else {
                        35.0
                    }),
                    target_speed: Some(if fixed {
                        *self.target.lock().unwrap()
                    } else {
                        35.0
                    }),
                    requested_speed: Some(if fixed {
                        *self.target.lock().unwrap()
                    } else {
                        -1.0
                    }),
                    speed_steps: Some(100),
                }],
                warnings: Vec::new(),
            })
        }
        fn set_auto(&self) -> Result<crate::util::CommandOutput, String> {
            if self.fail_auto {
                return Err("backend auto failure".into());
            }
            *self.fixed.lock().unwrap() = false;
            Ok(ok_output())
        }
        fn set_speed(&self, _fan: u32, percent: f64) -> Result<crate::util::CommandOutput, String> {
            *self.fixed.lock().unwrap() = true;
            *self.target.lock().unwrap() = percent;
            Ok(ok_output())
        }
        fn sensors_show(&self) -> Result<NbfcSensorConfig, String> {
            Ok(NbfcSensorConfig {
                fans: vec![NbfcFanSensor {
                    index: 0,
                    name: "CPU fan".into(),
                    sensors: vec!["@CPU".into()],
                    algorithm: Some("Average".into()),
                }],
            })
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
    fn daemon_reports_ready_and_stopping_with_socket_mode() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("neosensed.sock");
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
        assert!(error.contains("écoute déjà"));
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
        assert!(before >= 1, "un watchdog sain doit être émis");
        daemon.state.lock().unwrap().sensors.collected_at_ms = 0;
        thread::sleep(Duration::from_millis(1_200));
        let after = notifier
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| *e == "watchdog")
            .count();
        assert_eq!(
            before, after,
            "le watchdog doit cesser quand la santé est fausse"
        );
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
        assert!(health.reason.unwrap().contains("trop ancien"));

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
