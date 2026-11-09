use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use neosense::daemon::socket_path_from_env;
use neosense::diagnostics::{CheckGroup, CheckStatus, DiagnosticReport, diagnose_with_socket};
use neosense::ipc::{ApiError, Client};
use neosense::model::{ProviderError, SensorSample, SensorSnapshot, VERSION, now_ms};
use neosense::nbfc::{NbfcCapabilities, NbfcCli, NbfcSensorConfig, NbfcStatus};
use neosense::policy::fan_role;
use neosense::profiles::{
    ProfileAction, ProfileContext, ProfileEvaluation, evaluate, evaluate_all, find as find_profile,
};
use neosense::regulation::{RegulationMode, TargetDecision, evaluate_profile};
use neosense::runtime::{HostInfo, RuntimeView, fan_observations, host_info, read_view};
use neosense::sensors::{SysfsRoot, collect_all};
use neosense::service::{ServiceStatus, nbfc_service_status};
use neosense::supervisor::{SupervisorProbe, probe as probe_supervisor};
use neosense::warnings::{Severity, Warning, from_view as warnings_from_view};
use serde::Serialize;
use serde_json::Value;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "neosense",
    version = VERSION,
    about = "Thermal monitoring and safe fan management for Linux",
    long_about = "NeoSense monitors sensors and delegates every fan write to the neosensed supervisor. Read commands work without the supervisor."
)]
struct Cli {
    /// Stable JSON output on stdout; diagnostics on stderr.
    #[arg(long, global = true)]
    json: bool,
    /// Disable colour (NO_COLOR is also honoured).
    #[arg(long, global = true)]
    no_color: bool,
    /// Supervisor Unix socket.
    #[arg(long, global = true, env = "NEOSENSE_SOCKET")]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Summary of thermal state, backend and fans.
    Status,
    /// Sensor inventory and details.
    Sensors {
        #[command(subcommand)]
        command: SensorsCommand,
    },
    /// Fan status and control (writes only via the supervisor).
    Fan {
        #[command(subcommand)]
        command: FanCommand,
    },
    /// NeoSense thermal profiles.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// NBFC hardware configurations (inspect/validate only).
    Hardware {
        #[command(subcommand)]
        command: HardwareCommand,
    },
    /// System service status.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Read-only compatibility diagnostics.
    Diagnose,
    /// Effective NeoSense settings.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Interactive terminal UI (read-only without a supervisor).
    Tui,
    /// Generate shell completions.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
    /// NeoSense version.
    Version,
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Show effective settings (no hidden user config file).
    Show,
    /// Show the effective supervisor socket path.
    Path,
}

#[derive(Subcommand, Debug)]
enum SensorsCommand {
    /// List every discovered sensor.
    List,
    /// Show a sensor by stable identifier.
    Show { id: String },
    /// Periodic read-only refresh.
    Watch {
        /// Seconds between samples (minimum 1).
        #[arg(long, default_value_t = 2)]
        interval: u64,
        /// Print one sample and exit; useful for scripts and tests.
        #[arg(long)]
        once: bool,
    },
}

#[derive(Subcommand, Debug)]
enum FanCommand {
    /// Fan state and sensor provenance.
    Status,
    /// Return to NBFC automatic mode (safe, via the supervisor).
    Auto,
    /// Request a manual fan speed (subject to the safety policy).
    Set {
        /// cpu, gpu, fan name, or index.
        fan: String,
        /// Percentage 0–100 (the policy enforces a floor).
        percent: f64,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Hold the current target for one or all fans.
    Hold {
        /// cpu, gpu, fan name, or index. Default: all.
        fan: Option<String>,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Explicit return to firmware/EC control (not validated in this version).
    Release {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ProfileCommand {
    /// List available profiles.
    List,
    /// Show profile details.
    Show { name: String },
    /// Apply a profile (only safe-auto is currently applicable).
    Apply {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Calculate reference fan targets without a supervisor or hardware write.
    Preview { name: String },
    /// Select a closed-loop profile for supervisor simulation only.
    Select { name: String },
    /// Clear the supervisor simulation selection without writing fan state.
    Clear,
}

#[derive(Subcommand, Debug)]
enum HardwareCommand {
    /// List NBFC configurations.
    List,
    /// Currently selected NBFC configuration.
    Current,
    /// Analyse a configuration with nbfc rate-config (read-only).
    Validate { name: String },
}

#[derive(Subcommand, Debug)]
enum ServiceCommand {
    /// Status of nbfc_service and the NeoSense supervisor.
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
            "unsupported_profile" => 9,
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
                .map_err(|e| CliError::new(1, format!("JSON serialization: {e}")))?
        );
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct ProfilePreviewReport {
    schema: &'static str,
    profile: String,
    mode: &'static str,
    fan_targets: Vec<TargetDecision>,
    notes: Vec<String>,
    hardware_applied: bool,
}

#[derive(Debug, Serialize)]
struct ProfileReport<'a> {
    schema: &'static str,
    #[serde(flatten)]
    evaluation: &'a ProfileEvaluation,
}

#[derive(Debug, Serialize)]
struct SensorsReport<'a> {
    schema: &'static str,
    #[serde(flatten)]
    snapshot: &'a SensorSnapshot,
}

#[derive(Debug, Serialize)]
struct SensorReport<'a> {
    schema: &'static str,
    #[serde(flatten)]
    sensor: &'a SensorSample,
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
    supervisor: SupervisorProbe,
    warnings: Vec<Warning>,
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
                eprintln!("{} {}", ui.error("ERROR"), error.message);
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
        Command::Status => command_status(cli, ui),
        Command::Sensors { command } => command_sensors(ui, command),
        Command::Fan { command } => command_fan(cli, ui, command),
        Command::Profile { command } => command_profile(cli, ui, command),
        Command::Hardware { command } => command_hardware(ui, command),
        Command::Service { command } => command_service(cli, ui, command),
        Command::Diagnose => command_diagnose(cli, ui),
        Command::Config { command } => command_config(cli, ui, command),
        Command::Tui => {
            if cli.json {
                return Err(CliError::new(2, "--json is not compatible with the TUI"));
            }
            if cli.no_color {
                return Err(CliError::new(
                    2,
                    "--no-color is not supported by the interactive TUI; use CLI commands for colour-free output",
                ));
            }
            let socket = cli.socket.clone().unwrap_or_else(socket_path_from_env);
            neosense::tui::run(Some(socket)).map_err(|e| CliError::new(1, e))?;
            Ok(0)
        }
        Command::Completions { shell } => {
            if cli.json {
                return Err(CliError::new(
                    2,
                    "--json is not compatible with shell completion output",
                ));
            }
            let mut command = Cli::command();
            clap_complete::generate(*shell, &mut command, "neosense", &mut std::io::stdout());
            Ok(0)
        }
        Command::Version => {
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "name": "neosense",
                    "version": VERSION,
                    "schema": "neosense.version/v1",
                }))?;
            } else {
                println!("NeoSense {VERSION}");
                println!("Fan control is delegated to neosensed (NBFC).");
            }
            Ok(0)
        }
    }
}

fn command_status(cli: &Cli, ui: &Ui) -> Result<u8, CliError> {
    let view = read_view();
    let supervisor = probe_supervisor(&effective_socket(cli));
    let warnings = warnings_from_view(&view, Some(&supervisor));
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
        supervisor,
        warnings: warnings.clone(),
    };

    if ui.json {
        ui.print_json(&report)?;
    } else {
        print_status_human(ui, &view);
        if !warnings.is_empty() {
            println!("\n{}", ui.heading("Warnings"));
            for warning in warnings.iter().take(4) {
                let style = match warning.severity {
                    Severity::Critical => ui.error("CRITICAL"),
                    Severity::Warning => ui.warn("WARNING"),
                    Severity::Info => ui.info("INFO"),
                    Severity::Success => ui.ok("OK"),
                };
                println!("  [{style}] {} — {}", warning.title, warning.detail);
                println!("           next: {}", warning.action);
            }
        }
    }

    if view.backend_error.is_some() {
        Ok(7)
    } else {
        Ok(0)
    }
}

fn print_status_human(ui: &Ui, view: &RuntimeView) {
    let host = host_info();
    let host_label = format!(
        "{} {}",
        host.vendor.unwrap_or_else(|| "Machine".into()),
        host.model.unwrap_or_else(|| "unknown".into())
    );
    println!(
        "{}  {}  {}",
        ui.heading(&format!("NeoSense {VERSION}")),
        host_label,
        view.version
            .map(|v| format!("NBFC {v}"))
            .unwrap_or_else(|| "NBFC unavailable".into())
    );
    println!(
        "Service   {}  ·  Config {}",
        ui.ok(if view.service.is_active() {
            "running"
        } else {
            "inactive"
        }),
        view.status
            .as_ref()
            .and_then(|s| s.selected_config.as_deref())
            .unwrap_or("none")
    );
    let cpu = view.snapshot.cpu_package(now_ms(), 3_000);
    println!(
        "CPU       {}",
        cpu.map(SensorSample::display_value)
            .unwrap_or_else(|| ui.error("unavailable"))
    );
    let gpus = view.snapshot.gpu_indexes();
    if gpus.is_empty() {
        println!("GPU       {}", ui.warn("no sensor identified"));
    } else {
        for index in gpus {
            let value = view
                .snapshot
                .gpu_die(index, now_ms(), 3_000)
                .map(SensorSample::display_value)
                .unwrap_or_else(|| "unavailable".into());
            println!("GPU {index}     {value}");
        }
    }

    if let Some(status) = &view.status {
        println!("\n{}", ui.heading("Fans"));
        for fan in &status.fans {
            let mode = match fan.auto_mode {
                Some(true) => "auto",
                Some(false) => "manual",
                None => "unknown",
            };
            let target = fan
                .target_speed
                .map(|v| format!("{v:.1}%"))
                .unwrap_or_else(|| "n/a".into());
            let current = fan
                .current_speed
                .map(|v| format!("{v:.1}%"))
                .unwrap_or_else(|| "n/a".into());
            let source = view
                .sensor_config
                .as_ref()
                .and_then(|cfg| cfg.fans.iter().find(|f| f.index == fan.index))
                .map(|f| f.sensors.join(","))
                .unwrap_or_else(|| "unknown".into());
            let implausible = fan
                .current_speed
                .map(|v| !(0.0..=100.0).contains(&v))
                .unwrap_or(false)
                || matches!(
                    (fan.current_speed, fan.target_speed),
                    (Some(cur), Some(tgt)) if (cur - tgt).abs() > 20.0
                );
            println!(
                "  {:<10} {:<7} target {:<7} EC {:<7}{} source {}{}",
                fan.name,
                mode,
                target,
                current,
                if implausible { " (!)" } else { "" },
                source,
                if fan.critical == Some(true) {
                    ui.warn("  CRITICAL")
                } else {
                    String::new()
                }
            );
        }
    }

    if let Some(error) = &view.backend_error {
        println!("\n{} {}", ui.error("ERROR"), error);
    }
    if let Some(status) = &view.status
        && let Some(error) = status.warnings.first()
    {
        println!("{} {}", ui.warn("WARNING"), error);
    }
    println!(
        "\n{}",
        ui.info("EC speeds are not RPM. Use `neosense diagnose` for provenance.")
    );
}

fn command_sensors(ui: &Ui, command: &SensorsCommand) -> Result<u8, CliError> {
    match command {
        SensorsCommand::List => {
            let snapshot = collect_all(&SysfsRoot::default());
            if ui.json {
                ui.print_json(&SensorsReport {
                    schema: "neosense.sensors/v1",
                    snapshot: &snapshot,
                })?;
            } else {
                print_sensors(ui, &snapshot);
            }
            Ok(0)
        }
        SensorsCommand::Show { id } => {
            let snapshot = collect_all(&SysfsRoot::default());
            let Some(sensor) = snapshot.sensors.iter().find(|s| s.id == *id) else {
                return Err(CliError::new(3, format!("sensor not found: {id}")));
            };
            if ui.json {
                ui.print_json(&SensorReport {
                    schema: "neosense.sensor/v1",
                    sensor,
                })?;
            } else {
                println!("{}", ui.heading(&sensor.id));
                println!("role      {}", sensor.role);
                println!("value     {}", sensor.display_value());
                println!(
                    "raw       {} {}",
                    sensor.raw_value.as_deref().unwrap_or("-"),
                    sensor.raw_unit
                );
                println!("source    {} / {}", sensor.provider, sensor.chip);
                println!("path      {}", sensor.path.as_deref().unwrap_or("-"));
                println!("label     {}", sensor.label.as_deref().unwrap_or("-"));
                println!("trust     {:?}", sensor.health.trust);
                println!("status    {:?}", sensor.status);
                if let Some(error) = &sensor.error {
                    println!("error     {error}");
                }
            }
            Ok(0)
        }
        SensorsCommand::Watch { interval, once } => {
            if *interval == 0 {
                return Err(CliError::new(2, "--interval must be at least 1 second"));
            }
            let delay = Duration::from_secs(*interval);
            loop {
                let snapshot = collect_all(&SysfsRoot::default());
                if ui.json {
                    println!(
                        "{}",
                        serde_json::to_string(&SensorsReport {
                            schema: "neosense.sensors/v1",
                            snapshot: &snapshot,
                        })
                        .map_err(|e| CliError::new(1, format!("JSON serialization: {e}")))?
                    );
                } else {
                    if !*once && ui.color && std::io::stdout().is_terminal() {
                        print!("\x1b[2J\x1b[H");
                    }
                    print_sensors(ui, &snapshot);
                    if !*once {
                        println!("\n{}", ui.info("Ctrl-C to quit."));
                    }
                }
                let _ = std::io::stdout().flush();
                if *once {
                    break;
                }
                thread::sleep(delay);
            }
            Ok(0)
        }
    }
}

fn print_sensors(ui: &Ui, snapshot: &SensorSnapshot) {
    println!("{}", ui.heading("Sensors"));
    println!(
        "{:<24} {:<18} {:<12} {:<11} {:<12} Path / label",
        "Role", "Value", "Trust", "Status", "Source"
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
            ui.warn("WARNING"),
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
                return Err(CliError::new(2, "percentage must be between 0 and 100"));
            }
            if !yes && !dry_run {
                return Err(CliError::new(
                    8,
                    "manual control refused: add --yes after checking temperatures",
                ));
            }
            let view = read_view();
            let Some(status) = &view.status else {
                return Err(CliError::new(7, "NBFC status unavailable"));
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
        FanCommand::Hold { fan, yes, dry_run } => {
            if !yes && !dry_run {
                return Err(CliError::new(
                    8,
                    "manual hold refused: add --yes after checking temperatures",
                ));
            }
            let view = read_view();
            let Some(status) = &view.status else {
                return Err(CliError::new(7, "NBFC status unavailable"));
            };
            let indexes: Vec<u32> = if let Some(target) = fan {
                vec![resolve_fan(status, target)?]
            } else {
                status.fans.iter().map(|f| f.index).collect()
            };
            if indexes.is_empty() {
                return Err(CliError::new(7, "no NBFC fan available"));
            }
            // Resolve every target before the first write so a missing or
            // ambiguous fan cannot leave an earlier fan changed.
            let mut plans = Vec::with_capacity(indexes.len());
            for index in indexes {
                let Some(nbfc_fan) = status.fan(index) else {
                    return Err(CliError::new(3, format!("fan {index} not found")));
                };
                let target = nbfc_fan
                    .requested_speed
                    .filter(|value| *value >= 0.0)
                    .or(nbfc_fan.target_speed)
                    .ok_or_else(|| {
                        CliError::new(9, format!("fan {index}: no reliable current target"))
                    })?;
                plans.push((index, target));
            }

            let mut client = daemon_client(cli)?;
            for (index, target) in plans {
                match client.call(
                    "fan.set",
                    serde_json::json!({
                        "fan": index,
                        "percent": target,
                        "confirm": true,
                        "dry_run": dry_run,
                    }),
                ) {
                    Ok(result) => print_control(ui, &result)?,
                    Err(error) if *dry_run => return Err(error.into()),
                    Err(error) => {
                        let recovery = client.call("fan.auto", serde_json::json!({}));
                        let message = match recovery {
                            Ok(_) => format!(
                                "fan {index} hold failed: {}; automatic recovery requested",
                                error.message
                            ),
                            Err(recovery_error) => format!(
                                "fan {index} hold failed: {}; automatic recovery also failed: {}",
                                error.message, recovery_error.message
                            ),
                        };
                        return Err(CliError::new(10, message));
                    }
                }
            }
            Ok(0)
        }
        FanCommand::Release { yes } => {
            if !yes {
                return Err(CliError::new(
                    8,
                    "release requires --yes and is not implemented without reset-ec validation",
                ));
            }
            Err(CliError::new(
                9,
                "firmware/EC release is not implemented: requires reset-ec/NBFC stopped and hardware validation",
            ))
        }
    }
}

fn print_fan_status(ui: &Ui, view: &RuntimeView) {
    if let Some(error) = &view.backend_error {
        println!("{} {}", ui.error("ERROR"), error);
        return;
    }
    let Some(status) = &view.status else {
        println!("{}", ui.error("NBFC status unavailable"));
        return;
    };
    println!("{}", ui.heading("NBFC fans"));
    println!(
        "{:<6} {:<12} {:<8} {:<8} {:<10} {:<9} {:<9} {:<10} Source",
        "Index", "Name", "Role", "Mode", "Requested", "Target", "EC", "RPM"
    );
    for fan in &status.fans {
        let source = view
            .sensor_config
            .as_ref()
            .and_then(|cfg| cfg.fans.iter().find(|f| f.index == fan.index))
            .map(|f| f.sensors.join(","))
            .unwrap_or_else(|| "unknown".into());
        println!(
            "{:<6} {:<12} {:<8} {:<8} {:<10} {:<9} {:<9} {:<10} {}",
            fan.index,
            fan.name,
            fan_role(&fan.name).to_string(),
            match fan.auto_mode {
                Some(true) => "auto",
                Some(false) => "manual",
                None => "unknown",
            },
            opt_percent(fan.requested_speed),
            opt_percent(fan.target_speed),
            opt_percent(fan.current_speed),
            "n/a",
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
        println!("RPM: unavailable (no hwmon tachometer)");
    } else {
        for rpm in rpms {
            println!(
                "RPM {}: {} ({})",
                rpm.role,
                rpm.display_value(),
                rpm.label.as_deref().unwrap_or("no label")
            );
        }
    }
}

fn opt_percent(value: Option<f64>) -> String {
    value
        .filter(|v| *v >= 0.0)
        .map(|v| format!("{v:.1}%"))
        .unwrap_or_else(|| "n/a".into())
}

fn resolve_fan(status: &NbfcStatus, target: &str) -> Result<u32, CliError> {
    if let Ok(index) = target.parse::<u32>() {
        if status.fan(index).is_some() {
            return Ok(index);
        }
        return Err(CliError::new(3, format!("fan {index} not found")));
    }
    let lower = target.to_ascii_lowercase();
    let matches: Vec<&neosense::nbfc::NbfcFan> = status
        .fans
        .iter()
        .filter(|fan| fan.name.to_ascii_lowercase().contains(&lower))
        .collect();
    match matches.as_slice() {
        [fan] => Ok(fan.index),
        [] => Err(CliError::new(3, format!("fan not found: {target}"))),
        _ => Err(CliError::new(3, format!("multiple fans match {target:?}"))),
    }
}

fn command_profile(cli: &Cli, ui: &Ui, command: &ProfileCommand) -> Result<u8, CliError> {
    match command {
        ProfileCommand::List => {
            let view = read_view();
            let context = ProfileContext::from_view(&view);
            let evaluations = evaluate_all(&context);
            if ui.json {
                ui.print_json(&evaluations)?;
            } else {
                println!("{}", ui.heading("Thermal profiles"));
                for evaluation in &evaluations {
                    println!(
                        "  {:<12} {:<22} {:<26} {}",
                        evaluation.profile.name,
                        evaluation.profile.title,
                        evaluation.compatibility.label(),
                        if evaluation.compatibility.can_apply() {
                            "applicable"
                        } else {
                            "not applicable"
                        }
                    );
                    for reason in &evaluation.reasons {
                        println!("               reason: {reason}");
                    }
                    for warning in &evaluation.warnings {
                        println!("               warning: {warning}");
                    }
                }
            }
            Ok(0)
        }
        ProfileCommand::Show { name } => {
            let Some(profile) = find_profile(name) else {
                return Err(CliError::new(3, format!("profile not found: {name}")));
            };
            let view = read_view();
            let evaluation = evaluate(profile, &ProfileContext::from_view(&view));
            if ui.json {
                ui.print_json(&ProfileReport {
                    schema: "neosense.profile/v1",
                    evaluation: &evaluation,
                })?;
            } else {
                println!("{} — {}", evaluation.profile.name, evaluation.profile.title);
                println!("{}", evaluation.profile.description);
                println!("compatibility: {}", evaluation.compatibility.label());
                println!("action: {:?}", evaluation.profile.action);
                println!("safety: {}", evaluation.profile.safety);
                for reason in &evaluation.reasons {
                    println!("reason: {reason}");
                }
                for warning in &evaluation.warnings {
                    println!("warning: {warning}");
                }
            }
            Ok(0)
        }
        ProfileCommand::Preview { name } => {
            let Some(profile) = find_profile(name) else {
                return Err(CliError::new(3, format!("profile not found: {name}")));
            };
            let Some(curve) = profile.curve else {
                return Err(CliError::new(
                    9,
                    format!("profile {name:?} has no validated reference curve"),
                ));
            };
            let view = read_view();
            let Some(status) = &view.status else {
                return Err(CliError::new(7, "NBFC status unavailable"));
            };
            let fans = fan_observations(&view);
            let targets = evaluate_profile(
                curve,
                &view.snapshot,
                &fans,
                RegulationMode::Preview,
                now_ms(),
            )
            .map_err(|e| CliError::new(9, e))?;
            let report = ProfilePreviewReport {
                schema: "neosense.profile_preview/v1",
                profile: name.clone(),
                mode: "preview",
                fan_targets: targets,
                notes: vec![
                    "Reference curve for simulation only; not validated on this hardware.".into(),
                    "No fan write occurred. Hardware control remains unavailable.".into(),
                    format!("NBFC fans reported: {}", status.fans.len()),
                ],
                hardware_applied: false,
            };
            if ui.json {
                ui.print_json(&report)?;
            } else {
                println!("{} — {} (preview)", profile.name, profile.title);
                println!("{}", ui.info("Preview only: no fan write performed."));
                println!(
                    "{}",
                    ui.info("Reference curves are not hardware-validated.")
                );
                for target in &report.fan_targets {
                    match target.target_percent {
                        Some(percent) => println!(
                            "  {:<12} {:>5.1} °C -> {:.1}% ({})",
                            target.fan_name,
                            target.temperature_c.unwrap_or(f64::NAN),
                            percent,
                            target.mode.label()
                        ),
                        None => println!(
                            "  {:<12} unavailable — {}",
                            target.fan_name,
                            target.reason.as_deref().unwrap_or("no trustworthy data")
                        ),
                    }
                }
            }
            Ok(0)
        }
        ProfileCommand::Select { name } => {
            let mut client = daemon_client(cli)?;
            let result = client.call("profile.select", serde_json::json!({ "name": name }))?;
            if ui.json {
                ui.print_json(&result)?;
            } else {
                println!(
                    "{} profile {name} selected for simulation; no fan write performed.",
                    ui.ok("OK")
                );
                if let Some(targets) = result
                    .get("profile")
                    .and_then(|p| p.get("targets"))
                    .and_then(Value::as_array)
                {
                    for target in targets {
                        let fan = target
                            .get("fan_name")
                            .and_then(Value::as_str)
                            .unwrap_or("fan");
                        match target.get("target_percent").and_then(Value::as_f64) {
                            Some(percent) => {
                                println!("  {fan}: {:.1}% simulated (no hardware write)", percent)
                            }
                            None => println!(
                                "  {fan}: unavailable — {}",
                                target
                                    .get("reason")
                                    .and_then(Value::as_str)
                                    .unwrap_or("no trustworthy data")
                            ),
                        }
                    }
                }
            }
            Ok(0)
        }
        ProfileCommand::Clear => {
            let mut client = daemon_client(cli)?;
            let result = client.call("profile.clear", serde_json::json!({}))?;
            if ui.json {
                ui.print_json(&result)?;
            } else {
                println!(
                    "{} profile simulation selection cleared; no fan write performed.",
                    ui.ok("OK")
                );
            }
            Ok(0)
        }
        ProfileCommand::Apply { name, yes } => {
            let Some(profile) = find_profile(name) else {
                return Err(CliError::new(3, format!("profile not found: {name}")));
            };
            let view = read_view();
            let evaluation = evaluate(profile, &ProfileContext::from_view(&view));
            if !evaluation.compatibility.can_apply()
                || evaluation.profile.action != ProfileAction::SafeAuto
            {
                let mut message = format!(
                    "profile {name:?} is not applicable: {}",
                    evaluation.compatibility.label()
                );
                if !evaluation.reasons.is_empty() {
                    message.push_str(" — ");
                    message.push_str(&evaluation.reasons.join("; "));
                }
                return Err(CliError::new(9, message));
            }
            if !yes {
                return Err(CliError::new(
                    8,
                    "apply safe-auto with --yes (returns to NBFC automatic mode)",
                ));
            }
            let mut client = daemon_client(cli)?;
            let result = client.call("fan.auto", serde_json::json!({}))?;
            print_control(ui, &result)?;
            Ok(0)
        }
    }
}

fn command_hardware(ui: &Ui, command: &HardwareCommand) -> Result<u8, CliError> {
    let backend = NbfcCli::default();
    match command {
        HardwareCommand::List => {
            let configs = backend.list_configs().map_err(|e| CliError::new(7, e))?;
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.hardware_list/v1",
                    "configs": configs,
                }))?;
            } else {
                println!("{}", ui.heading("NBFC configurations"));
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
                let mut value = serde_json::to_value(&status)
                    .map_err(|e| CliError::new(1, format!("JSON serialization: {e}")))?;
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "schema".to_string(),
                        serde_json::Value::String("neosense.hardware_status/v1".to_string()),
                    );
                }
                ui.print_json(&value)?;
            } else {
                println!(
                    "{}",
                    status
                        .selected_config
                        .as_deref()
                        .unwrap_or("no configuration")
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
                println!("{}", ui.heading(&format!("Analysis of {name}")));
                println!("{value}");
            }
            Ok(0)
        }
    }
}

fn command_service(cli: &Cli, ui: &Ui, command: &ServiceCommand) -> Result<u8, CliError> {
    match command {
        ServiceCommand::Status => {
            let status = nbfc_service_status();
            let socket = effective_socket(cli);
            let supervisor = probe_supervisor(&socket);
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.service_status/v2",
                    "nbfc": status,
                    "supervisor": supervisor,
                }))?;
            } else {
                println!("{}", ui.heading("Services"));
                println!(
                    "nbfc_service  {} / {}",
                    status.active_state.as_deref().unwrap_or("unknown"),
                    status.unit_file_state.as_deref().unwrap_or("unknown")
                );
                let status_label = match supervisor.state {
                    neosense::supervisor::SupervisorState::Healthy => ui.ok("healthy"),
                    neosense::supervisor::SupervisorState::Connected => ui.info("connected"),
                    neosense::supervisor::SupervisorState::Degraded => ui.warn("degraded"),
                    neosense::supervisor::SupervisorState::NotRunning => ui.error("not running"),
                    neosense::supervisor::SupervisorState::PermissionDenied => {
                        ui.error("permission denied")
                    }
                    neosense::supervisor::SupervisorState::StaleSocket => ui.error("stale socket"),
                    neosense::supervisor::SupervisorState::ProtocolError => {
                        ui.error("protocol error")
                    }
                };
                println!("neosensed     {} ({})", status_label, socket.display());
                println!(
                    "socket        mode {}, uid {}, gid {}",
                    supervisor.socket_mode.as_deref().unwrap_or("n/a"),
                    supervisor
                        .socket_uid
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "n/a".into()),
                    supervisor
                        .socket_gid
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "n/a".into())
                );
                if let Some(health) = supervisor.health {
                    println!(
                        "health        {}",
                        if health { "healthy" } else { "degraded" }
                    );
                }
                if let Some(reason) = &supervisor.reason {
                    println!("reason        {reason}");
                }
                if let Some(error) = &supervisor.error {
                    println!("detail        {error}");
                }
                println!("next          {}", supervisor.state.action());
                if let Some(error) = &status.error {
                    println!("{} {}", ui.error("ERROR"), error);
                }
            }
            Ok(match supervisor.state {
                neosense::supervisor::SupervisorState::Healthy
                | neosense::supervisor::SupervisorState::Connected => 0,
                neosense::supervisor::SupervisorState::Degraded => 6,
                neosense::supervisor::SupervisorState::PermissionDenied => 4,
                neosense::supervisor::SupervisorState::NotRunning
                | neosense::supervisor::SupervisorState::StaleSocket
                | neosense::supervisor::SupervisorState::ProtocolError => 7,
            })
        }
    }
}

fn command_config(cli: &Cli, ui: &Ui, command: &ConfigCommand) -> Result<u8, CliError> {
    let socket = effective_socket(cli);
    match command {
        ConfigCommand::Show => {
            let safety = neosense::policy::SafetyConfig::default();
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.config/v1",
                    "socket": socket,
                    "safety": {
                        "stale_after_ms": safety.stale_after_ms,
                        "manual_ttl_ms": safety.manual_ttl_ms,
                        "min_percent": safety.min_percent,
                        "max_temp_c": safety.max_temp_c,
                        "require_gpu_sensor_source": safety.require_gpu_sensor_source,
                    },
                    "note": "No user configuration file is loaded in this version."
                }))?;
            } else {
                println!("{}", ui.heading("Effective configuration"));
                println!("socket              {}", socket.display());
                println!("staleness           {} ms", safety.stale_after_ms);
                println!("manual lease        {} ms", safety.manual_ttl_ms);
                println!("manual floor        {} %", safety.min_percent);
                println!("manual ceiling      {:.1} °C", safety.max_temp_c);
                println!("GPU source required {}", safety.require_gpu_sensor_source);
                println!();
                println!("No user configuration file is loaded in this version.");
                println!("Values shown are compiled defaults.");
            }
            Ok(0)
        }
        ConfigCommand::Path => {
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.config_path/v1",
                    "socket": socket,
                }))?;
            } else {
                println!("{}", socket.display());
            }
            Ok(0)
        }
    }
}

fn command_diagnose(cli: &Cli, ui: &Ui) -> Result<u8, CliError> {
    let backend = NbfcCli::default();
    let snapshot = collect_all(&SysfsRoot::default());
    let report: DiagnosticReport =
        diagnose_with_socket(&backend, &snapshot, &effective_socket(cli));
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
    println!("{}", ui.heading("NeoSense diagnostics"));
    if let Some(os) = &report.environment.os {
        println!(
            "{} · kernel {} · {} {}",
            os,
            report.environment.kernel.as_deref().unwrap_or("?"),
            report.environment.vendor.as_deref().unwrap_or(""),
            report.environment.model.as_deref().unwrap_or("")
        );
    }

    let summary = &report.summary;
    println!("\n{}", ui.heading("Summary"));
    println!("  Operational       : {}", yes_no(summary.operational, ui));
    println!("  Supervisor        : {}", summary.supervisor.label());
    println!(
        "  Fan control       : {}",
        yes_no(summary.fan_control_available, ui)
    );
    println!(
        "  Thermal readings  : {}",
        if summary.thermal_reliable {
            ui.ok("reliable")
        } else {
            ui.warn("degraded")
        }
    );
    println!("  Critical issues   : {}", summary.critical_issues);
    println!("  Warnings          : {}", summary.warning_issues);
    println!(
        "  Overall           : {}",
        check_label(summary.overall, ui).trim()
    );
    if !summary.operational {
        println!(
            "  Next safe action  : resolve critical issues; use `neosense diagnose --json` for full detail."
        );
    }

    let criticals: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .collect();
    if !criticals.is_empty() {
        println!("\n{}", ui.heading("Critical issues / blockers"));
        for check in criticals {
            print_check(ui, check);
        }
    }

    for group in [
        CheckGroup::Backend,
        CheckGroup::Hardware,
        CheckGroup::Thermal,
        CheckGroup::Fans,
        CheckGroup::Control,
        CheckGroup::Security,
    ] {
        let checks: Vec<_> = report
            .checks_in(group)
            .filter(|check| check.status != CheckStatus::Fail)
            .collect();
        if checks.is_empty() {
            continue;
        }
        println!("\n{}", ui.heading(group.label()));
        for check in checks {
            print_check(ui, check);
        }
    }

    println!(
        "\n{}",
        ui.info("Use `neosense diagnose --json` for the complete machine-readable report.")
    );
}

fn print_check(ui: &Ui, check: &neosense::diagnostics::DiagnosticCheck) {
    let label = check_label(check.status, ui);
    println!("  [{label}] {:<28} {}", check.title, check.message);
}

fn check_label(status: CheckStatus, ui: &Ui) -> String {
    match status {
        CheckStatus::Ok => ui.ok("OK  "),
        CheckStatus::Info => ui.info("INFO"),
        CheckStatus::Warning => ui.warn("WARN"),
        CheckStatus::Fail => ui.error("CRIT"),
    }
}

fn yes_no(value: bool, ui: &Ui) -> String {
    if value { ui.ok("yes") } else { ui.error("no") }
}

fn effective_socket(cli: &Cli) -> PathBuf {
    cli.socket.clone().unwrap_or_else(socket_path_from_env)
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
        .unwrap_or("command executed");
    let dry = value
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    println!(
        "{} {}",
        if verified {
            ui.ok("OK")
        } else {
            ui.warn("UNVERIFIED")
        },
        message
    );
    if dry {
        println!("{}", ui.info("Dry-run: no write performed."));
    }
    if let Some(reported) = value.get("reported_percent").and_then(Value::as_f64) {
        let plausible = value
            .get("readback_plausible")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        println!(
            "Reported EC speed: {reported:.1}% ({})",
            if plausible { "plausible" } else { "unreliable" }
        );
    }
    Ok(())
}
