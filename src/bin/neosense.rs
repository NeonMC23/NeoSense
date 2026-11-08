use clap::{Parser, Subcommand};
use neosense::daemon::socket_path_from_env;
use neosense::diagnostics::{CheckStatus, DiagnosticReport, diagnose};
use neosense::ipc::{ApiError, Client};
use neosense::model::{ProviderError, SensorSample, SensorSnapshot, VERSION, now_ms};
use neosense::nbfc::{NbfcCapabilities, NbfcCli, NbfcSensorConfig, NbfcStatus, NbfcVersion};
use neosense::policy::fan_role;
use neosense::sensors::{SysfsRoot, collect_all};
use neosense::service::{ServiceStatus, nbfc_service_status};
use serde::Serialize;
use serde_json::Value;
use std::io::{IsTerminal, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "neosense",
    version = VERSION,
    about = "Surveillance thermique et gestion sûre des ventilateurs pour Linux",
    long_about = "NeoSense surveille les capteurs et délègue toute écriture de ventilateur au superviseur neosensed. Les commandes de lecture fonctionnent sans superviseur."
)]
struct Cli {
    /// Sortie JSON stable sur stdout, diagnostics sur stderr.
    #[arg(long, global = true)]
    json: bool,
    /// Désactive la couleur (NO_COLOR est aussi respecté).
    #[arg(long, global = true)]
    no_color: bool,
    /// Socket Unix du superviseur.
    #[arg(long, global = true, env = "NEOSENSE_SOCKET")]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Résumé de l'état thermique, du backend et des ventilateurs.
    Status,
    /// Inventaire et détails des capteurs.
    Sensors {
        #[command(subcommand)]
        command: SensorsCommand,
    },
    /// État et contrôle des ventilateurs (contrôle via le superviseur).
    Fan {
        #[command(subcommand)]
        command: FanCommand,
    },
    /// Profils thermiques NeoSense.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Configurations matérielles NBFC (lecture/validation seulement).
    Hardware {
        #[command(subcommand)]
        command: HardwareCommand,
    },
    /// État des services système.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Diagnostic de compatibilité en lecture seule.
    Diagnose,
    /// Version de NeoSense.
    Version,
}

#[derive(Subcommand, Debug)]
enum SensorsCommand {
    /// Liste tous les capteurs découverts.
    List,
    /// Détails d'un capteur par identifiant stable.
    Show { id: String },
    /// Rafraîchissement périodique en lecture seule.
    Watch {
        #[arg(long, default_value_t = 2)]
        interval: u64,
    },
}

#[derive(Subcommand, Debug)]
enum FanCommand {
    /// État des ventilateurs et provenance des capteurs.
    Status,
    /// Retour au mode automatique NBFC (sûr, via le superviseur).
    Auto,
    /// Demande de vitesse manuelle (soumise à la politique de sécurité).
    Set {
        /// cpu, gpu, nom du ventilateur, ou index.
        fan: String,
        /// Pourcentage 0–100 (la politique impose un plancher).
        percent: f64,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ProfileCommand {
    /// Liste les profils disponibles.
    List,
    /// Détails d'un profil.
    Show { name: String },
    /// Applique un profil (safe-auto uniquement pour l'instant).
    Apply {
        name: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
enum HardwareCommand {
    /// Liste les configurations NBFC.
    List,
    /// Configuration NBFC actuellement sélectionnée.
    Current,
    /// Analyse une configuration avec nbfc rate-config (lecture seule).
    Validate { name: String },
}

#[derive(Subcommand, Debug)]
enum ServiceCommand {
    /// État de nbfc_service et du superviseur NeoSense.
    Status,
}

#[derive(Debug, Clone)]
struct CliError {
    code: u8,
    message: String,
    details: Vec<String>,
}

impl CliError {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: Vec::new(),
        }
    }
}

impl From<ApiError> for CliError {
    fn from(error: ApiError) -> Self {
        let code = match error.code.as_str() {
            "unsafe_refused" | "confirmation_required" => 8,
            "permission_denied" => 4,
            "backend_unavailable"
            | "backend_failed"
            | "ipc_connect"
            | "ipc_config"
            | "ipc_read"
            | "ipc_write" => 7,
            "verification_failed" => 10,
            "bad_request" | "unknown_method" => 2,
            _ => 1,
        };
        Self {
            code,
            message: error.message,
            details: error.details,
        }
    }
}

struct Ui {
    json: bool,
    color: bool,
}

impl Ui {
    fn new(cli: &Cli) -> Self {
        let color = !cli.no_color
            && std::env::var_os("NO_COLOR").is_none()
            && std::io::stdout().is_terminal();
        Self {
            json: cli.json,
            color,
        }
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn heading(&self, text: &str) -> String {
        self.paint("1;36", text)
    }

    fn ok(&self, text: &str) -> String {
        self.paint("32", text)
    }

    fn warn(&self, text: &str) -> String {
        self.paint("33", text)
    }

    fn error(&self, text: &str) -> String {
        self.paint("31", text)
    }

    fn info(&self, text: &str) -> String {
        self.paint("36", text)
    }

    fn print_json<T: Serialize>(&self, value: &T) -> Result<(), CliError> {
        println!(
            "{}",
            serde_json::to_string_pretty(value)
                .map_err(|e| CliError::new(1, format!("sérialisation JSON: {e}")))?
        );
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct StatusReport {
    schema: &'static str,
    generated_at_ms: u64,
    version: &'static str,
    host: HostInfo,
    nbfc_version: Option<String>,
    capabilities: Option<NbfcCapabilities>,
    service: ServiceStatus,
    nbfc: Option<NbfcStatus>,
    sensors: Vec<SensorSample>,
    sensor_errors: Vec<ProviderError>,
    sensor_config: Option<NbfcSensorConfig>,
    backend_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HostInfo {
    vendor: Option<String>,
    model: Option<String>,
    bios: Option<String>,
    kernel: Option<String>,
}

struct View {
    snapshot: SensorSnapshot,
    version: Option<NbfcVersion>,
    capabilities: Option<NbfcCapabilities>,
    status: Option<NbfcStatus>,
    service: ServiceStatus,
    sensor_config: Option<NbfcSensorConfig>,
    backend_error: Option<String>,
}

fn read_view() -> View {
    let backend = NbfcCli::default();
    let snapshot = collect_all(&SysfsRoot::default());
    let version = backend.version().ok();
    let capabilities = version.map(NbfcCapabilities::from_version);
    let (status, backend_error) = match (version, &capabilities) {
        (Some(version), Some(_)) => match backend.status(version) {
            Ok(status) => (Some(status), None),
            Err(e) => (None, Some(e)),
        },
        _ => (None, Some("NBFC indisponible".to_string())),
    };
    View {
        snapshot,
        version,
        capabilities,
        status,
        service: nbfc_service_status(),
        sensor_config: backend.sensors_show().ok(),
        backend_error,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ui = Ui::new(&cli);
    match run(&cli, &ui) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if ui.json {
                let payload = serde_json::json!({
                    "error": {
                        "code": error.code,
                        "message": error.message,
                        "details": error.details,
                    }
                });
                let _ = writeln!(std::io::stdout(), "{payload}");
            } else {
                eprintln!("{} {}", ui.error("ERREUR"), error.message);
                for detail in &error.details {
                    eprintln!("  - {detail}");
                }
            }
            ExitCode::from(error.code)
        }
    }
}

fn run(cli: &Cli, ui: &Ui) -> Result<u8, CliError> {
    match &cli.command {
        Command::Status => command_status(ui),
        Command::Sensors { command } => command_sensors(ui, command),
        Command::Fan { command } => command_fan(cli, ui, command),
        Command::Profile { command } => command_profile(cli, ui, command),
        Command::Hardware { command } => command_hardware(ui, command),
        Command::Service { command } => command_service(ui, command),
        Command::Diagnose => command_diagnose(ui),
        Command::Version => {
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "name": "neosense",
                    "version": VERSION,
                    "schema": "neosense.version/v1",
                }))?;
            } else {
                println!("NeoSense {VERSION}");
                println!("Contrôle des ventilateurs délégué à neosensed (NBFC).");
            }
            Ok(0)
        }
    }
}

fn command_status(ui: &Ui) -> Result<u8, CliError> {
    let view = read_view();
    let report = StatusReport {
        schema: "neosense.status/v1",
        generated_at_ms: now_ms(),
        version: VERSION,
        host: host_info(),
        nbfc_version: view.version.map(|v| v.to_string()),
        capabilities: view.capabilities.clone(),
        service: view.service.clone(),
        nbfc: view.status.clone(),
        sensors: view.snapshot.sensors.clone(),
        sensor_errors: view.snapshot.errors.clone(),
        sensor_config: view.sensor_config.clone(),
        backend_error: view.backend_error.clone(),
    };

    if ui.json {
        ui.print_json(&report)?;
    } else {
        print_status_human(ui, &view);
    }

    if view.backend_error.is_some() {
        Ok(7)
    } else {
        Ok(0)
    }
}

fn print_status_human(ui: &Ui, view: &View) {
    let host = host_info();
    let host_label = format!(
        "{} {}",
        host.vendor.unwrap_or_else(|| "Machine".into()),
        host.model.unwrap_or_else(|| "inconnue".into())
    );
    println!(
        "{}  {}  {}",
        ui.heading(&format!("NeoSense {VERSION}")),
        host_label,
        view.version
            .map(|v| format!("NBFC {v}"))
            .unwrap_or_else(|| "NBFC indisponible".into())
    );
    println!(
        "Service   {}  ·  Config {}",
        ui.ok(if view.service.is_active() {
            "actif"
        } else {
            "inactif"
        }),
        view.status
            .as_ref()
            .and_then(|s| s.selected_config.as_deref())
            .unwrap_or("aucune")
    );
    let cpu = view.snapshot.cpu_package(now_ms(), 3_000);
    println!(
        "CPU       {}",
        cpu.map(SensorSample::display_value)
            .unwrap_or_else(|| ui.error("indisponible"))
    );
    let gpus = view.snapshot.gpu_indexes();
    if gpus.is_empty() {
        println!("GPU       {}", ui.warn("aucun capteur identifié"));
    } else {
        for index in gpus {
            let value = view
                .snapshot
                .gpu_die(index, now_ms(), 3_000)
                .map(SensorSample::display_value)
                .unwrap_or_else(|| "indisponible".into());
            println!("GPU {index}     {value}");
        }
    }

    if let Some(status) = &view.status {
        println!("\n{}", ui.heading("Ventilateurs"));
        for fan in &status.fans {
            let mode = match fan.auto_mode {
                Some(true) => "auto",
                Some(false) => "manuel",
                None => "inconnu",
            };
            let target = fan
                .target_speed
                .map(|v| format!("{v:.1}%"))
                .unwrap_or_else(|| "?".into());
            let current = fan
                .current_speed
                .map(|v| format!("{v:.1}%"))
                .unwrap_or_else(|| "?".into());
            let source = view
                .sensor_config
                .as_ref()
                .and_then(|cfg| cfg.fans.iter().find(|f| f.index == fan.index))
                .map(|f| f.sensors.join(","))
                .unwrap_or_else(|| "?".into());
            let implausible = fan
                .current_speed
                .map(|v| !(0.0..=100.0).contains(&v))
                .unwrap_or(false)
                || matches!(
                    (fan.current_speed, fan.target_speed),
                    (Some(cur), Some(tgt)) if (cur - tgt).abs() > 50.0
                );
            println!(
                "  {:<10} {:<7} cible {:<7} EC {:<7}{} source {}{}",
                fan.name,
                mode,
                target,
                current,
                if implausible { " (!)" } else { "" },
                source,
                if fan.critical == Some(true) {
                    ui.warn("  CRITIQUE")
                } else {
                    String::new()
                }
            );
        }
    }

    if let Some(error) = &view.backend_error {
        println!("\n{} {}", ui.error("ERREUR"), error);
    }
    if let Some(status) = &view.status
        && let Some(error) = status.warnings.first()
    {
        println!("{} {}", ui.warn("ATTENTION"), error);
    }
    println!(
        "\n{}",
        ui.info(
            "Les vitesses EC ne sont pas des RPM. Utilisez `neosense diagnose` pour la provenance."
        )
    );
}

fn command_sensors(ui: &Ui, command: &SensorsCommand) -> Result<u8, CliError> {
    match command {
        SensorsCommand::List => {
            let snapshot = collect_all(&SysfsRoot::default());
            if ui.json {
                ui.print_json(&snapshot)?;
            } else {
                print_sensors(ui, &snapshot);
            }
            Ok(0)
        }
        SensorsCommand::Show { id } => {
            let snapshot = collect_all(&SysfsRoot::default());
            let Some(sensor) = snapshot.sensors.iter().find(|s| s.id == *id) else {
                return Err(CliError::new(3, format!("capteur introuvable: {id}")));
            };
            if ui.json {
                ui.print_json(sensor)?;
            } else {
                println!("{}", ui.heading(&sensor.id));
                println!("rôle      {}", sensor.role);
                println!("valeur    {}", sensor.display_value());
                println!(
                    "brut      {} {}",
                    sensor.raw_value.as_deref().unwrap_or("-"),
                    sensor.raw_unit
                );
                println!("source    {} / {}", sensor.provider, sensor.chip);
                println!("chemin    {}", sensor.path.as_deref().unwrap_or("-"));
                println!("label     {}", sensor.label.as_deref().unwrap_or("-"));
                println!("confiance {:?}", sensor.health.trust);
                println!("statut    {:?}", sensor.status);
                if let Some(error) = &sensor.error {
                    println!("erreur    {error}");
                }
            }
            Ok(0)
        }
        SensorsCommand::Watch { interval } => {
            let interval = Duration::from_secs((*interval).max(1));
            loop {
                let snapshot = collect_all(&SysfsRoot::default());
                if ui.color && std::io::stdout().is_terminal() {
                    print!("\x1b[2J\x1b[H");
                }
                print_sensors(ui, &snapshot);
                println!("\n{}", ui.info("Ctrl-C pour quitter."));
                let _ = std::io::stdout().flush();
                thread::sleep(interval);
            }
        }
    }
}

fn print_sensors(ui: &Ui, snapshot: &SensorSnapshot) {
    println!("{}", ui.heading("Capteurs"));
    println!(
        "{:<24} {:<18} {:<12} {:<11} {:<12} Chemin / label",
        "Rôle", "Valeur", "Confiance", "Statut", "Source"
    );
    for sensor in &snapshot.sensors {
        println!(
            "{:<24} {:<18} {:<12} {:<11} {:<12} {}",
            sensor.role.to_string(),
            sensor.display_value(),
            format!("{:?}", sensor.health.trust),
            format!("{:?}", sensor.status),
            sensor.provider.to_string(),
            sensor.path.as_deref().unwrap_or("-")
        );
    }
    for error in &snapshot.errors {
        println!(
            "{} {}: {}",
            ui.warn("ATTENTION"),
            error.provider,
            error.message
        );
    }
}

fn command_fan(cli: &Cli, ui: &Ui, command: &FanCommand) -> Result<u8, CliError> {
    match command {
        FanCommand::Status => {
            let view = read_view();
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.fan_status/v1",
                    "service": view.service,
                    "nbfc": view.status,
                    "sensor_config": view.sensor_config,
                    "sensors": view.snapshot,
                    "backend_error": view.backend_error,
                }))?;
            } else {
                print_fan_status(ui, &view);
            }
            Ok(if view.status.is_some() { 0 } else { 7 })
        }
        FanCommand::Auto => {
            let mut client = daemon_client(cli)?;
            let result = client.call("fan.auto", serde_json::json!({}))?;
            print_control(ui, &result)?;
            Ok(0)
        }
        FanCommand::Set {
            fan,
            percent,
            yes,
            dry_run,
        } => {
            if !percent.is_finite() || !(0.0..=100.0).contains(percent) {
                return Err(CliError::new(2, "le pourcentage doit être entre 0 et 100"));
            }
            if !yes && !dry_run {
                return Err(CliError::new(
                    8,
                    "contrôle manuel refusé: ajoutez --yes après avoir vérifié les températures",
                ));
            }
            let view = read_view();
            let Some(status) = &view.status else {
                return Err(CliError::new(7, "statut NBFC indisponible"));
            };
            let index = resolve_fan(status, fan)?;
            let mut client = daemon_client(cli)?;
            let result = client.call(
                "fan.set",
                serde_json::json!({
                    "fan": index,
                    "percent": percent,
                    "confirm": true,
                    "dry_run": dry_run,
                }),
            )?;
            print_control(ui, &result)?;
            Ok(0)
        }
    }
}

fn print_fan_status(ui: &Ui, view: &View) {
    if let Some(error) = &view.backend_error {
        println!("{} {}", ui.error("ERREUR"), error);
        return;
    }
    let Some(status) = &view.status else {
        println!("{}", ui.error("statut NBFC indisponible"));
        return;
    };
    println!("{}", ui.heading("Ventilateurs NBFC"));
    for fan in &status.fans {
        let source = view
            .sensor_config
            .as_ref()
            .and_then(|cfg| cfg.fans.iter().find(|f| f.index == fan.index))
            .map(|f| f.sensors.join(","))
            .unwrap_or_else(|| "?".into());
        println!(
            "#{} {:<12} rôle={:?} auto={:?} cible={:?} demandé={:?} EC={:?} source={}",
            fan.index,
            fan.name,
            fan_role(&fan.name),
            fan.auto_mode,
            fan.target_speed,
            fan.requested_speed,
            fan.current_speed,
            source
        );
    }
    let rpms: Vec<&SensorSample> = view
        .snapshot
        .sensors
        .iter()
        .filter(|s| matches!(s.role, neosense::model::SensorRole::FanRpm(_)))
        .collect();
    if rpms.is_empty() {
        println!("RPM: indisponible (aucun tachymètre hwmon)");
    } else {
        for rpm in rpms {
            println!(
                "RPM {}: {} ({})",
                rpm.role,
                rpm.display_value(),
                rpm.label.as_deref().unwrap_or("sans label")
            );
        }
    }
}

fn resolve_fan(status: &NbfcStatus, target: &str) -> Result<u32, CliError> {
    if let Ok(index) = target.parse::<u32>() {
        if status.fan(index).is_some() {
            return Ok(index);
        }
        return Err(CliError::new(3, format!("ventilateur {index} introuvable")));
    }
    let lower = target.to_ascii_lowercase();
    let matches: Vec<&neosense::nbfc::NbfcFan> = status
        .fans
        .iter()
        .filter(|fan| fan.name.to_ascii_lowercase().contains(&lower))
        .collect();
    match matches.as_slice() {
        [fan] => Ok(fan.index),
        [] => Err(CliError::new(
            3,
            format!("ventilateur introuvable: {target}"),
        )),
        _ => Err(CliError::new(
            3,
            format!("plusieurs ventilateurs correspondent à {target:?}"),
        )),
    }
}

fn command_profile(cli: &Cli, ui: &Ui, command: &ProfileCommand) -> Result<u8, CliError> {
    match command {
        ProfileCommand::List => {
            let profiles = profile_list();
            if ui.json {
                ui.print_json(&profiles)?;
            } else {
                println!("{}", ui.heading("Profils NeoSense"));
                for profile in &profiles {
                    println!("  {:<12} {}", profile.name, profile.description);
                }
            }
            Ok(0)
        }
        ProfileCommand::Show { name } => {
            let Some(profile) = profile_list().into_iter().find(|p| p.name == *name) else {
                return Err(CliError::new(3, format!("profil introuvable: {name}")));
            };
            if ui.json {
                ui.print_json(&profile)?;
            } else {
                println!("{}", profile.name);
                println!("{}", profile.description);
                println!("sécurité: {}", profile.safety);
            }
            Ok(0)
        }
        ProfileCommand::Apply { name, yes } => {
            if name != "safe-auto" {
                return Err(CliError::new(
                    9,
                    format!("profil {name:?} non disponible: seul safe-auto est implémenté"),
                ));
            }
            if !yes {
                return Err(CliError::new(
                    8,
                    "appliquez safe-auto avec --yes (retour au mode automatique NBFC)",
                ));
            }
            let mut client = daemon_client(cli)?;
            let result = client.call("fan.auto", serde_json::json!({}))?;
            print_control(ui, &result)?;
            Ok(0)
        }
    }
}

#[derive(Debug, Serialize)]
struct Profile {
    name: &'static str,
    description: &'static str,
    safety: &'static str,
}

fn profile_list() -> Vec<Profile> {
    vec![Profile {
        name: "safe-auto",
        description: "Retour au contrôle automatique NBFC (état sûr par défaut).",
        safety: "Aucune écriture manuelle; vérifie l'état auto après la commande.",
    }]
}

fn command_hardware(ui: &Ui, command: &HardwareCommand) -> Result<u8, CliError> {
    let backend = NbfcCli::default();
    match command {
        HardwareCommand::List => {
            let configs = backend.list_configs().map_err(|e| CliError::new(7, e))?;
            if ui.json {
                ui.print_json(&serde_json::json!({"configs": configs}))?;
            } else {
                println!("{}", ui.heading("Configurations NBFC"));
                for config in configs {
                    println!("  {config}");
                }
            }
            Ok(0)
        }
        HardwareCommand::Current => {
            let version = backend.version().map_err(|e| CliError::new(7, e))?;
            let status = backend.status(version).map_err(|e| CliError::new(7, e))?;
            if ui.json {
                ui.print_json(&status)?;
            } else {
                println!(
                    "{}",
                    status
                        .selected_config
                        .as_deref()
                        .unwrap_or("aucune configuration")
                );
            }
            Ok(0)
        }
        HardwareCommand::Validate { name } => {
            let value = backend.rate_config_json(name).map_err(|e| {
                if e.to_ascii_lowercase().contains("root") {
                    CliError::new(4, e)
                } else {
                    CliError::new(7, e)
                }
            })?;
            if ui.json {
                // `rate-config -j` already emits JSON; preserve it.
                print!("{value}");
                if !value.ends_with('\n') {
                    println!();
                }
            } else {
                println!("{}", ui.heading(&format!("Analyse de {name}")));
                println!("{value}");
            }
            Ok(0)
        }
    }
}

fn command_service(ui: &Ui, command: &ServiceCommand) -> Result<u8, CliError> {
    match command {
        ServiceCommand::Status => {
            let status = nbfc_service_status();
            let socket = socket_path_from_env();
            let (supervisor_status, supervisor_error) = match Client::connect(&socket) {
                Ok(mut client) => match client.call("status.get", serde_json::json!({})) {
                    Ok(value) => (Some(value), None),
                    Err(error) => (None, Some(error)),
                },
                Err(error) => (None, Some(error)),
            };
            let daemon_running = supervisor_status.is_some();
            let supervisor_health = supervisor_status
                .as_ref()
                .and_then(|value| value.get("health"))
                .map(|health| {
                    if health.get("healthy").and_then(Value::as_bool) == Some(true) {
                        "sain".to_string()
                    } else {
                        format!(
                            "dégradé: {}",
                            health
                                .get("reason")
                                .and_then(Value::as_str)
                                .unwrap_or("raison inconnue")
                        )
                    }
                });
            let socket_meta = std::fs::metadata(&socket).ok();
            let socket_info = socket_meta.as_ref().map(|meta| {
                serde_json::json!({
                    "mode": format!("{:o}", meta.permissions().mode() & 0o777),
                    "uid": meta.uid(),
                    "gid": meta.gid(),
                })
            });
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.service_status/v1",
                    "nbfc": status,
                    "supervisor": {
                        "socket": socket,
                        "socket_info": socket_info,
                        "running": daemon_running,
                        "status": supervisor_status,
                        "error": supervisor_error,
                    }
                }))?;
            } else {
                println!("{}", ui.heading("Services"));
                println!(
                    "nbfc_service  {} / {}",
                    status.active_state.as_deref().unwrap_or("inconnu"),
                    status.unit_file_state.as_deref().unwrap_or("inconnu")
                );
                println!(
                    "neosensed     {} ({})",
                    if daemon_running {
                        ui.ok("actif")
                    } else {
                        ui.warn("inactif")
                    },
                    socket.display()
                );
                if let Some(meta) = &socket_meta {
                    println!(
                        "socket        mode {:o}, uid {}, gid {}",
                        meta.permissions().mode() & 0o777,
                        meta.uid(),
                        meta.gid()
                    );
                }
                if let Some(health) = &supervisor_health {
                    println!("santé         {health}");
                }
                if let Some(error) = &supervisor_error {
                    println!("{} {}", ui.warn("SUPERVISEUR"), error.message);
                    for detail in &error.details {
                        println!("  - {detail}");
                    }
                }
                if let Some(error) = &status.error {
                    println!("{} {}", ui.error("ERREUR"), error);
                }
            }
            let permission_error = supervisor_error
                .as_ref()
                .is_some_and(|error| error.code == "permission_denied");
            Ok(if status.error.is_some() {
                7
            } else if permission_error {
                4
            } else {
                0
            })
        }
    }
}

fn command_diagnose(ui: &Ui) -> Result<u8, CliError> {
    let backend = NbfcCli::default();
    let snapshot = collect_all(&SysfsRoot::default());
    let report: DiagnosticReport = diagnose(&backend, &snapshot);
    if ui.json {
        ui.print_json(&report)?;
    } else {
        print_diagnostic(ui, &report);
    }
    Ok(match report.overall() {
        CheckStatus::Fail => 5,
        _ => 0,
    })
}

fn print_diagnostic(ui: &Ui, report: &DiagnosticReport) {
    println!("{}", ui.heading("Diagnostic NeoSense"));
    if let Some(os) = &report.environment.os {
        println!(
            "{} · kernel {} · {} {}",
            os,
            report.environment.kernel.as_deref().unwrap_or("?"),
            report.environment.vendor.as_deref().unwrap_or(""),
            report.environment.model.as_deref().unwrap_or("")
        );
    }
    for check in &report.checks {
        let label = match check.status {
            CheckStatus::Ok => ui.ok("OK  "),
            CheckStatus::Info => ui.info("INFO"),
            CheckStatus::Warning => ui.warn("WARN"),
            CheckStatus::Fail => ui.error("FAIL"),
        };
        println!("[{label}] {:<28} {}", check.title, check.message);
    }
    println!("\nRésultat: {:?}", report.overall());
}

fn daemon_client(cli: &Cli) -> Result<Client, CliError> {
    let path = cli.socket.clone().unwrap_or_else(socket_path_from_env);
    Client::connect(&path).map_err(CliError::from)
}

fn print_control(ui: &Ui, value: &Value) -> Result<(), CliError> {
    if ui.json {
        ui.print_json(value)?;
        return Ok(());
    }
    let verified = value
        .get("verified")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("commande exécutée");
    let dry = value
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    println!(
        "{} {}",
        if verified {
            ui.ok("OK")
        } else {
            ui.warn("NON VÉRIFIÉ")
        },
        message
    );
    if dry {
        println!("{}", ui.info("Mode dry-run: aucune écriture."));
    }
    if let Some(reported) = value.get("reported_percent").and_then(Value::as_f64) {
        let plausible = value
            .get("readback_plausible")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        println!(
            "Vitesse EC rapportée: {reported:.1}% ({})",
            if plausible { "plausible" } else { "non fiable" }
        );
    }
    Ok(())
}

fn host_info() -> HostInfo {
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
