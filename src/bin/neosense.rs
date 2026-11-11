use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use neosense::calibration::{CalibrationInput, analyze as analyze_calibration};
use neosense::control::{ControlReadiness, RequirementScope, RequirementStatus};
use neosense::custom_profiles::{
    CustomProfile, CustomStatus, Store as CustomStore, StoreError, editor_path, read_profile,
    write_private_file,
};
use neosense::daemon::socket_path_from_env;
use neosense::diagnostics::{CheckGroup, CheckStatus, DiagnosticReport, diagnose};
use neosense::ipc::{ApiError, Client};
use neosense::model::{ProviderError, SensorSample, SensorSnapshot, VERSION, now_ms};
use neosense::nbfc::{NbfcCapabilities, NbfcSensorConfig, NbfcStatus};
use neosense::nbfc_config;
use neosense::policy::{FanRole, fan_role};
use neosense::profiles::{
    ProfileAction, ProfileContext, ProfileEvaluation, ThermalProfile, custom_to_thermal, evaluate,
    evaluate_all_with, find as find_profile,
};
use neosense::regulation::{RegulationMode, TargetDecision, evaluate_profile};
use neosense::runtime::{HostInfo, RuntimeView, fan_observations, host_info, read_view};
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
    long_about = "NeoSense displays sensor and fan state supplied by the neosensed supervisor and delegates every fan write to it. The CLI never reads hardware or NBFC directly; without the supervisor, live data is explicitly unavailable."
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
    /// Historical fixed fan presets.
    Preset {
        #[command(subcommand)]
        command: PresetCommand,
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
    /// Read-only control-readiness assessment.
    Control {
        #[command(subcommand)]
        command: ControlCommand,
    },
    /// Passive P1 calibration analysis (no hardware access).
    Calibration {
        #[command(subcommand)]
        command: CalibrationCommand,
    },
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
enum ControlCommand {
    /// Explain why hardware control is or is not eligible.
    Readiness,
}

#[derive(Subcommand, Debug)]
enum CalibrationCommand {
    /// Analyze paired external-reference and Acer RPM samples.
    Analyze {
        /// JSON file containing `samples: [{reference, acer_fan1, acer_fan2}]`.
        #[arg(long)]
        input: PathBuf,
    },
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
    /// Show the current supervisor regulation state.
    State,
    /// Clear the supervisor simulation selection without writing fan state.
    Clear,
    /// Create, inspect and edit persistent user thermal curves.
    Custom {
        #[command(subcommand)]
        command: CustomProfileCommand,
    },
}

#[derive(Subcommand, Debug)]
enum CustomProfileCommand {
    /// List persisted custom thermal curves.
    List,
    /// Show one custom curve.
    Show { name: String },
    /// Create a curve from flags or a JSON file.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value = "")]
        description: String,
        /// TEMP:PERCENT point, repeated at least twice (for example 40:25).
        #[arg(long = "point", value_name = "TEMP:PERCENT")]
        points: Vec<String>,
        #[arg(long, default_value_t = 20.0)]
        min_percent: f64,
        #[arg(long, default_value_t = 100.0)]
        max_percent: f64,
        #[arg(long, default_value_t = 3.0)]
        hysteresis: f64,
        #[arg(long, default_value_t = 10.0)]
        max_step: f64,
        #[arg(long, default_value_t = 3000)]
        stale_ms: u64,
        /// Fan roles: cpu, gpu, or cpu,gpu.
        #[arg(long, value_delimiter = ',', default_value = "cpu,gpu")]
        fans: Vec<String>,
        /// Read a complete JSON profile definition instead of flags.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Replace an existing curve. Unspecified flags keep their current value.
    Update {
        name: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long = "point", value_name = "TEMP:PERCENT")]
        points: Vec<String>,
        #[arg(long)]
        min_percent: Option<f64>,
        #[arg(long)]
        max_percent: Option<f64>,
        #[arg(long)]
        hysteresis: Option<f64>,
        #[arg(long)]
        max_step: Option<f64>,
        #[arg(long)]
        stale_ms: Option<u64>,
        #[arg(long, value_delimiter = ',')]
        fans: Vec<String>,
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Copy an existing curve under a new name.
    Duplicate { from: String, to: String },
    /// Rename an existing curve.
    Rename { from: String, to: String },
    /// Delete a curve after explicit confirmation.
    Delete {
        name: String,
        #[arg(long)]
        yes: bool,
    },
    /// Validate a JSON profile file without writing it.
    Validate {
        #[arg(long)]
        file: PathBuf,
    },
    /// Edit a curve with $EDITOR, then validate and save it.
    Edit { name: String },
    /// Show the persistent store path.
    Path,
    /// Explicitly preserve a corrupt store and start an empty one.
    Reset {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
enum PresetCommand {
    /// List the historical fixed fan presets.
    List,
    /// Show a fixed preset's historical target values.
    Show { name: String },
    /// Apply a fixed preset through the supervisor and safety policy.
    Apply {
        name: String,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum HardwareCommand {
    /// List NBFC configurations.
    List,
    /// Currently selected NBFC configuration.
    Current,
    /// Analyse a configuration with nbfc rate-config (read-only).
    Validate { name: String },
    /// Show per-fan registers, sensor sources and readback capability.
    Identity {
        /// Configuration name; defaults to the selected NBFC configuration.
        #[arg(long)]
        config: Option<String>,
    },
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
            "permission_denied" | "root_required" => 4,
            "analysis_unavailable"
            | "backend_unavailable"
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
    /// `supervisor` when values came over IPC; `unavailable` when the client
    /// refused to substitute a local hardware read.
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_error: Option<&'a str>,
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
    sensor_source_error: Option<String>,
    sensor_config: Option<NbfcSensorConfig>,
    backend_error: Option<String>,
    supervisor: SupervisorProbe,
    control: ControlReadiness,
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
        Command::Sensors { command } => command_sensors(cli, ui, command),
        Command::Fan { command } => command_fan(cli, ui, command),
        Command::Profile { command } => command_profile(cli, ui, command),
        Command::Preset { command } => command_preset(ui, cli, command),
        Command::Hardware { command } => command_hardware(cli, ui, command),
        Command::Service { command } => command_service(cli, ui, command),
        Command::Diagnose => command_diagnose(cli, ui),
        Command::Control { command } => command_control(cli, ui, command),
        Command::Calibration { command } => command_calibration(ui, command),
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
    let view = read_view(&effective_socket(cli));
    let supervisor = view.supervisor.clone();
    let warnings = warnings_from_view(&view, Some(&supervisor));
    let control = neosense::diagnostics::control_readiness(&view);
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
        sensor_source_error: view.sensor_source_error.clone(),
        sensor_config: view.sensor_config.clone(),
        backend_error: view.backend_error.clone(),
        supervisor,
        control: control.clone(),
        warnings: warnings.clone(),
    };

    if ui.json {
        ui.print_json(&report)?;
    } else {
        print_status_human(ui, &view, &control);
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

fn print_status_human(ui: &Ui, view: &RuntimeView, control: &ControlReadiness) {
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
            .unwrap_or_else(|| {
                if view.supervisor.state.is_connected() {
                    "NBFC unavailable".into()
                } else {
                    format!("supervisor {}", view.supervisor.state.label())
                }
            })
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
    if let Some(error) = &view.sensor_source_error {
        println!("Sensors   {} {error}", ui.error("unavailable:"));
    }
    println!(
        "Control   {}  ·  manual path {}  ·  all fans {}  ·  physical control {}",
        control.stage.label(),
        if control.manual_path_eligible {
            "eligible"
        } else {
            "blocked"
        },
        if control.all_fans_eligible {
            "yes"
        } else {
            "no"
        },
        if control.physical_control_validated {
            "validated"
        } else {
            "not validated"
        }
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

    let now = now_ms();
    let rpms: Vec<&SensorSample> = view.snapshot.rpm_sensors().collect();
    println!("\n{}", ui.heading("RPM feedback"));
    if rpms.is_empty() {
        if view.sensor_source_error.is_some() {
            println!(
                "  {}",
                ui.error("unavailable (no sensor snapshot from the supervisor)")
            );
        } else {
            println!("  {}", ui.warn("unavailable (no independent tachometer)"));
        }
    } else {
        for rpm in rpms {
            let feedback = rpm.is_usable_for_feedback(now, 3_000);
            let fresh = rpm.value.is_some() && rpm.is_fresh(now, 3_000);
            println!(
                "  {:<14} {:<10} {:<10} {:<12} {}",
                rpm.display_name(),
                rpm.display_value(),
                rpm.health.trust.short_label(),
                if feedback {
                    "feedback"
                } else if fresh {
                    "not eligible"
                } else {
                    "stale/unavailable"
                },
                rpm.path.as_deref().unwrap_or("-")
            );
        }
        println!(
            "{}",
            ui.info("RPM is physical feedback only; CPU/GPU fan identity is not verified.")
        );
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
        ui.info(
            "EC percentages are not RPM. RPM feedback is separate and does not validate physical fan control."
        )
    );
}

fn command_sensors(cli: &Cli, ui: &Ui, command: &SensorsCommand) -> Result<u8, CliError> {
    let socket = effective_socket(cli);
    match command {
        SensorsCommand::List => {
            let view = read_view(&socket);
            let source = if view.sensor_source_error.is_none() {
                "supervisor"
            } else {
                "unavailable"
            };
            if ui.json {
                ui.print_json(&SensorsReport {
                    schema: "neosense.sensors/v1",
                    source,
                    source_error: view.sensor_source_error.as_deref(),
                    snapshot: &view.snapshot,
                })?;
            } else {
                if let Some(error) = &view.sensor_source_error {
                    println!("{} {error}", ui.warn("SOURCE"));
                }
                print_sensors(ui, &view.snapshot);
            }
            Ok(0)
        }
        SensorsCommand::Show { id } => {
            let view = read_view(&socket);
            let Some(sensor) = view.snapshot.sensors.iter().find(|s| s.id == *id) else {
                if let Some(error) = &view.sensor_source_error {
                    return Err(CliError::new(7, error.clone()));
                }
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
                let view = read_view(&socket);
                let source = if view.sensor_source_error.is_none() {
                    "supervisor"
                } else {
                    "unavailable"
                };
                if ui.json {
                    println!(
                        "{}",
                        serde_json::to_string(&SensorsReport {
                            schema: "neosense.sensors/v1",
                            source,
                            source_error: view.sensor_source_error.as_deref(),
                            snapshot: &view.snapshot,
                        })
                        .map_err(|e| CliError::new(1, format!("JSON serialization: {e}")))?
                    );
                } else {
                    if !*once && ui.color && std::io::stdout().is_terminal() {
                        print!("\x1b[2J\x1b[H");
                    }
                    if let Some(error) = &view.sensor_source_error {
                        println!("{} {error}", ui.warn("SOURCE"));
                    }
                    print_sensors(ui, &view.snapshot);
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
            sensor.health.trust.short_label(),
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
            let view = read_view(&effective_socket(cli));
            if ui.json {
                let now = now_ms();
                let rpm_feedback: Vec<serde_json::Value> = view
                    .snapshot
                    .rpm_sensors()
                    .map(|sensor| {
                        serde_json::json!({
                            "index": match sensor.role {
                                neosense::model::SensorRole::FanRpm(index) => index,
                                _ => 0,
                            },
                            "name": sensor.display_name(),
                            "chip": &sensor.chip,
                            "path": &sensor.path,
                            "value": sensor.value,
                            "unit": "rpm",
                            "raw_value": &sensor.raw_value,
                            "status": sensor.status,
                            "trust": sensor.health.trust,
                            "fresh": sensor.value.is_some()
                                && sensor.is_fresh(now, 3_000),
                            "physical_measurement": sensor.health.physical_measurement,
                            "usable_for_feedback": sensor.is_usable_for_feedback(now, 3_000),
                            "usable_for_regulation": sensor.health.usable_for_regulation,
                        })
                    })
                    .collect();
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.fan_status/v1",
                    "service": view.service,
                    "nbfc": view.status,
                    "sensor_config": view.sensor_config,
                    "sensors": view.snapshot,
                    "rpm_feedback": rpm_feedback,
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
            let view = read_view(&effective_socket(cli));
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
            let view = read_view(&effective_socket(cli));
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
    println!(
        "{}",
        ui.info("Per-NBFC-fan RPM is not mapped: Acer hwmon channels have no CPU/GPU labels.")
    );
    let now = now_ms();
    let rpms: Vec<&SensorSample> = view.snapshot.rpm_sensors().collect();
    if rpms.is_empty() {
        println!("RPM: unavailable (no independent tachometer)");
    } else {
        for rpm in rpms {
            let feedback = rpm.is_usable_for_feedback(now, 3_000);
            let fresh = rpm.value.is_some() && rpm.is_fresh(now, 3_000);
            println!(
                "RPM {}: {} ({}; {}; {})",
                rpm.display_name(),
                rpm.display_value(),
                rpm.health.trust.short_label(),
                if feedback {
                    "feedback-eligible"
                } else {
                    "not feedback-eligible"
                },
                if fresh { "fresh" } else { "stale" }
            );
        }
        println!(
            "{}",
            ui.info(
                "Acer RPM channels are physical measurements; CPU/GPU fan identity is unverified."
            )
        );
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

fn command_preset(ui: &Ui, cli: &Cli, command: &PresetCommand) -> Result<u8, CliError> {
    match command {
        PresetCommand::List => {
            let presets = neosense::presets::builtins();
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.preset_list/v1",
                    "presets": presets,
                }))?;
            } else {
                println!("{}", ui.heading("Historical fixed presets"));
                for preset in presets {
                    println!(
                        "  {:<12} CPU {:>5.1}%  GPU {:>5.1}%  {}",
                        preset.name, preset.cpu_percent, preset.gpu_percent, preset.title
                    );
                }
                println!();
                println!(
                    "{}",
                    ui.info("Presets request fixed targets through the supervisor; they are not dynamic curves.")
                );
            }
            Ok(0)
        }
        PresetCommand::Show { name } => {
            let Some(preset) = neosense::presets::find(name) else {
                return Err(CliError::new(3, format!("preset not found: {name}")));
            };
            if ui.json {
                ui.print_json(&preset)?;
            } else {
                println!("{} — {}", preset.name, preset.title);
                println!("{}", preset.description);
                println!("CPU target  {:.1}%", preset.cpu_percent);
                println!("GPU target  {:.1}%", preset.gpu_percent);
                println!("safety      {}", preset.safety);
                println!();
                println!(
                    "{}",
                    ui.warn("Fixed preset: requires supervisor and validated backend.")
                );
            }
            Ok(0)
        }
        PresetCommand::Apply { name, yes, dry_run } => {
            if !yes && !dry_run {
                return Err(CliError::new(
                    8,
                    "preset application refused: add --yes after checking temperatures",
                ));
            }
            if neosense::presets::find(name).is_none() {
                return Err(CliError::new(3, format!("preset not found: {name}")));
            }
            let mut client = daemon_client(cli)?;
            let result = client.call(
                "preset.apply",
                serde_json::json!({
                    "name": name,
                    "confirm": true,
                    "dry_run": dry_run,
                }),
            )?;
            if ui.json {
                ui.print_json(&result)?;
            } else {
                println!(
                    "{} preset {name} applied through supervisor; targets confirmed by backend.",
                    ui.ok("OK")
                );
                if let Some(targets) = result.get("targets").and_then(Value::as_array) {
                    for target in targets {
                        let fan = target
                            .get("fan_name")
                            .and_then(Value::as_str)
                            .unwrap_or("fan");
                        let requested = target
                            .get("requested_percent")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0);
                        let accepted = target
                            .get("accepted_target_percent")
                            .and_then(Value::as_f64)
                            .map(|v| format!("{v:.1}%"))
                            .unwrap_or_else(|| "n/a".into());
                        println!("  {fan:<12} requested {requested:.1}%  accepted {accepted}");
                    }
                }
                println!(
                    "{}",
                    ui.info("EC readback is not RPM; physical motion is not proven.")
                );
            }
            Ok(0)
        }
    }
}

fn store_cli_error(error: StoreError) -> CliError {
    let code = match error.code() {
        "profile_not_found" | "config_conflict" => 3,
        "config_io" => 7,
        "config_invalid" | "config_corrupt" => 9,
        _ => 1,
    };
    CliError::new(code, error.to_string())
}

fn optional_client(cli: &Cli) -> Option<Client> {
    let path = cli.socket.clone().unwrap_or_else(socket_path_from_env);
    Client::connect(&path).ok()
}

fn custom_status(cli: &Cli) -> CustomStatus {
    if let Some(mut client) = optional_client(cli) {
        if let Ok(value) = client.call("custom.list", serde_json::json!({})) {
            if let Ok(status) = serde_json::from_value::<CustomStatus>(value) {
                return status;
            }
        }
    }
    CustomStore::client().status()
}

fn custom_profile(cli: &Cli, name: &str) -> Result<CustomProfile, CliError> {
    if let Some(mut client) = optional_client(cli) {
        match client.call("custom.get", serde_json::json!({ "name": name })) {
            Ok(value) => {
                return value
                    .get("profile")
                    .cloned()
                    .ok_or_else(|| CliError::new(1, "supervisor returned no profile"))
                    .and_then(|value| {
                        serde_json::from_value(value).map_err(|e| {
                            CliError::new(1, format!("invalid supervisor profile: {e}"))
                        })
                    });
            }
            Err(error) if error.code == "unknown_method" => {}
            Err(error) => return Err(error.into()),
        }
    }
    read_profile(&CustomStore::client(), name).map_err(store_cli_error)
}

fn resolve_profile_for_cli(cli: &Cli, name: &str) -> Result<ThermalProfile, CliError> {
    if let Some(profile) = find_profile(name) {
        return Ok(profile);
    }
    let custom = custom_profile(cli, name)?;
    Ok(custom_to_thermal(&custom))
}

fn parse_points(values: &[String]) -> Result<Vec<neosense::regulation::CurvePoint>, CliError> {
    if values.len() < 2 {
        return Err(CliError::new(
            9,
            "a custom curve needs at least two --point TEMP:PERCENT values",
        ));
    }
    values
        .iter()
        .map(|value| {
            let (temperature, percent) = value.split_once(':').ok_or_else(|| {
                CliError::new(9, format!("invalid point '{value}'; expected TEMP:PERCENT"))
            })?;
            let temperature_c = temperature
                .trim()
                .parse::<f64>()
                .map_err(|_| CliError::new(9, format!("invalid temperature in point '{value}'")))?;
            let percent = percent
                .trim()
                .trim_end_matches('%')
                .parse::<f64>()
                .map_err(|_| CliError::new(9, format!("invalid percentage in point '{value}'")))?;
            Ok(neosense::regulation::CurvePoint {
                temperature_c,
                percent,
            })
        })
        .collect()
}

fn parse_fan_roles(values: &[String]) -> Result<Vec<FanRole>, CliError> {
    if values.is_empty() {
        return Err(CliError::new(9, "at least one fan role is required"));
    }
    values
        .iter()
        .map(|value| match value.trim().to_ascii_lowercase().as_str() {
            "cpu" => Ok(FanRole::Cpu),
            "gpu" => Ok(FanRole::Gpu),
            other => Err(CliError::new(
                9,
                format!("unsupported fan role '{other}'; use cpu or gpu"),
            )),
        })
        .collect()
}

fn read_custom_file(path: &std::path::Path) -> Result<CustomProfile, CliError> {
    let text = if path == std::path::Path::new("-") {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
            .map_err(|e| CliError::new(1, format!("read stdin: {e}")))?;
        text
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::new(1, format!("read {}: {e}", path.display())))?
    };
    let profile: CustomProfile = serde_json::from_str(&text)
        .map_err(|e| CliError::new(9, format!("invalid profile JSON: {e}")))?;
    profile.validate().map_err(|e| CliError::new(9, e))?;
    Ok(profile)
}

fn local_custom_result(action: &str, status: CustomStatus, warnings: Vec<String>) -> Value {
    serde_json::json!({
        "schema": "neosense.custom_profile_result/v1",
        "action": action,
        "status": status,
        "warnings": warnings,
    })
}

fn submit_custom_create(cli: &Cli, profile: CustomProfile) -> Result<Value, CliError> {
    if let Some(mut client) = optional_client(cli) {
        match client.call("custom.create", serde_json::json!({ "profile": profile })) {
            Ok(value) => return Ok(value),
            Err(error) if error.code == "unknown_method" => {}
            Err(error) => return Err(error.into()),
        }
    }
    let store = CustomStore::client();
    let loaded = store.create(profile).map_err(store_cli_error)?;
    Ok(local_custom_result(
        "created",
        store.status(),
        loaded.warnings,
    ))
}

fn submit_custom_update(cli: &Cli, profile: CustomProfile) -> Result<Value, CliError> {
    if let Some(mut client) = optional_client(cli) {
        match client.call("custom.update", serde_json::json!({ "profile": profile })) {
            Ok(value) => return Ok(value),
            Err(error) if error.code == "unknown_method" => {}
            Err(error) => return Err(error.into()),
        }
    }
    let store = CustomStore::client();
    let loaded = store.update(profile).map_err(store_cli_error)?;
    Ok(local_custom_result(
        "updated",
        store.status(),
        loaded.warnings,
    ))
}

fn submit_custom_simple(
    cli: &Cli,
    method: &str,
    params: Value,
    local: impl FnOnce(&CustomStore) -> Result<CustomStatus, StoreError>,
    action: &str,
) -> Result<Value, CliError> {
    if let Some(mut client) = optional_client(cli) {
        match client.call(method, params) {
            Ok(value) => return Ok(value),
            Err(error) if error.code == "unknown_method" => {}
            Err(error) => return Err(error.into()),
        }
    }
    let store = CustomStore::client();
    let status = local(&store).map_err(store_cli_error)?;
    Ok(local_custom_result(action, status, Vec::new()))
}

fn print_custom_result(ui: &Ui, value: &Value) -> Result<(), CliError> {
    if ui.json {
        ui.print_json(value)?;
        return Ok(());
    }
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("updated");
    println!("{} custom profile {action}", ui.ok("OK"));
    if let Some(path) = value
        .get("status")
        .and_then(|status| status.get("path"))
        .and_then(Value::as_str)
    {
        println!("store        {path}");
    }
    if let Some(profiles) = value
        .get("status")
        .and_then(|status| status.get("profiles"))
        .and_then(Value::as_array)
    {
        if profiles.is_empty() {
            println!("profiles     none");
        } else {
            for profile in profiles {
                let name = profile
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("profile");
                let title = profile.get("title").and_then(Value::as_str).unwrap_or("");
                println!("  {name:<24} {title}");
            }
        }
    }
    if let Some(preserved) = value.get("preserved").and_then(Value::as_str) {
        println!("preserved    {preserved}");
    }
    if let Some(warnings) = value.get("warnings").and_then(Value::as_array) {
        for warning in warnings {
            if let Some(warning) = warning.as_str() {
                println!("{}", ui.warn(warning));
            }
        }
    }
    if let Some(error) = value
        .get("status")
        .and_then(|status| status.get("error"))
        .and_then(Value::as_str)
    {
        println!("{} {error}", ui.warn("store error:"));
    }
    Ok(())
}

fn print_custom_status(ui: &Ui, status: &CustomStatus) -> Result<(), CliError> {
    if ui.json {
        ui.print_json(status)?;
    } else {
        println!("{}", ui.heading("Custom thermal profiles"));
        println!("store        {}", status.path);
        if let Some(error) = &status.error {
            println!("{} {error}", ui.warn("store error:"));
        }
        for warning in &status.warnings {
            println!("{} {warning}", ui.warn("warning:"));
        }
        if status.profiles.is_empty() {
            println!("profiles     none");
        } else {
            for profile in &status.profiles {
                println!(
                    "  {:<24} {:<24} {} points  {:.0}–{:.0}%  fans {}",
                    profile.name,
                    profile.title,
                    profile.points.len(),
                    profile.min_percent,
                    profile.max_percent,
                    profile
                        .fan_roles
                        .iter()
                        .map(|role| role.to_string().to_ascii_lowercase())
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
        }
        println!();
        println!(
            "{}",
            ui.info("Custom profiles are dynamic curves; they are simulation/preview only.")
        );
    }
    Ok(())
}

fn command_profile(cli: &Cli, ui: &Ui, command: &ProfileCommand) -> Result<u8, CliError> {
    match command {
        ProfileCommand::List => {
            let view = read_view(&effective_socket(cli));
            let context = ProfileContext::from_view(&view);
            let custom = custom_status(cli);
            let evaluations = evaluate_all_with(&context, &custom.profiles);
            if ui.json {
                ui.print_json(&evaluations)?;
            } else {
                println!("{}", ui.heading("Thermal profiles"));
                for evaluation in &evaluations {
                    println!(
                        "  {:<20} {:<22} {:<20} {:<8} {}",
                        evaluation.profile.name,
                        evaluation.profile.title,
                        evaluation.compatibility.label(),
                        format!("{:?}", evaluation.profile.origin).to_ascii_lowercase(),
                        if evaluation.compatibility.can_apply() {
                            "applicable"
                        } else {
                            "not applicable"
                        }
                    );
                    println!(
                        "                       fans: eligible {:?}, blocked {:?}",
                        evaluation.eligible_fans, evaluation.blocked_fans
                    );
                    for reason in &evaluation.reasons {
                        println!("                       reason: {reason}");
                    }
                    for warning in &evaluation.warnings {
                        println!("                       warning: {warning}");
                    }
                }
                println!();
                println!("custom store: {}", custom.path);
                if let Some(error) = &custom.error {
                    println!("{} {error}", ui.warn("custom store error:"));
                }
                for warning in &custom.warnings {
                    println!("{} {warning}", ui.warn("warning:"));
                }
            }
            Ok(0)
        }
        ProfileCommand::Show { name } => {
            let profile = resolve_profile_for_cli(cli, name)?;
            let view = read_view(&effective_socket(cli));
            let evaluation = evaluate(profile, &ProfileContext::from_view(&view));
            if ui.json {
                ui.print_json(&ProfileReport {
                    schema: "neosense.profile/v1",
                    evaluation: &evaluation,
                })?;
            } else {
                println!("{} — {}", evaluation.profile.name, evaluation.profile.title);
                println!("{}", evaluation.profile.description);
                println!("origin: {:?}", evaluation.profile.origin);
                println!("compatibility: {}", evaluation.compatibility.label());
                println!("action: {:?}", evaluation.profile.action);
                println!("safety: {}", evaluation.profile.safety);
                println!(
                    "fans: eligible {:?}, blocked {:?}",
                    evaluation.eligible_fans, evaluation.blocked_fans
                );
                if let Some(curve) = &evaluation.profile.curve {
                    for point in &curve.points {
                        println!(
                            "  {:>5.1} °C -> {:>5.1}%",
                            point.temperature_c, point.percent
                        );
                    }
                }
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
            let profile = resolve_profile_for_cli(cli, name)?;
            let Some(curve) = profile.curve.clone() else {
                return Err(CliError::new(
                    9,
                    format!("profile {name:?} has no validated reference curve"),
                ));
            };
            let view = read_view(&effective_socket(cli));
            let Some(status) = &view.status else {
                return Err(CliError::new(7, "NBFC status unavailable"));
            };
            let fans: Vec<_> = fan_observations(&view)
                .into_iter()
                .filter(|fan| {
                    profile
                        .fan_roles
                        .as_ref()
                        .is_none_or(|roles| roles.contains(&fan.role))
                })
                .collect();
            let targets = evaluate_profile(
                &curve,
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
                    "Curve for simulation only; not validated on this hardware.".into(),
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
                    ui.info("Reference and custom curves are not hardware-validated.")
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
        ProfileCommand::State => {
            let mut client = daemon_client(cli)?;
            let result = client.call("profile.get", serde_json::json!({}))?;
            if ui.json {
                ui.print_json(&result)?;
            } else {
                let profile = result.get("profile").cloned().unwrap_or(Value::Null);
                let selected = profile
                    .get("selected")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                let mode = profile
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                println!("Profile     {selected} ({mode})");
                if let Some(reason) = profile.get("reason").and_then(Value::as_str) {
                    println!("Reason      {reason}");
                }
                if let Some(targets) = profile.get("targets").and_then(Value::as_array) {
                    for target in targets {
                        let name = target
                            .get("fan_name")
                            .and_then(Value::as_str)
                            .unwrap_or("fan");
                        match target.get("target_percent").and_then(Value::as_f64) {
                            Some(percent) => {
                                println!("{name:<12} {percent:.1}% simulated; no hardware write")
                            }
                            None => println!(
                                "{name:<12} unavailable — {}",
                                target
                                    .get("reason")
                                    .and_then(Value::as_str)
                                    .unwrap_or("no data")
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
            let view = read_view(&effective_socket(cli));
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
        ProfileCommand::Custom { command } => command_custom_profile(cli, ui, command),
    }
}

fn command_custom_profile(
    cli: &Cli,
    ui: &Ui,
    command: &CustomProfileCommand,
) -> Result<u8, CliError> {
    match command {
        CustomProfileCommand::List => {
            let status = custom_status(cli);
            print_custom_status(ui, &status)?;
            if status.error.is_some() { Ok(9) } else { Ok(0) }
        }
        CustomProfileCommand::Show { name } => {
            let profile = custom_profile(cli, name)?;
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.custom_profile/v1",
                    "profile": profile,
                }))?;
            } else {
                println!("{} — {}", profile.name, profile.title);
                if !profile.description.is_empty() {
                    println!("{}", profile.description);
                }
                println!(
                    "bounds       {:.0}–{:.0}%  hysteresis {:.1} °C  max step {:.1}%  stale {} ms",
                    profile.min_percent,
                    profile.max_percent,
                    profile.hysteresis_c,
                    profile.max_step_percent,
                    profile.stale_after_ms
                );
                println!(
                    "fans         {}",
                    profile
                        .fan_roles
                        .iter()
                        .map(|role| role.to_string().to_ascii_lowercase())
                        .collect::<Vec<_>>()
                        .join(",")
                );
                for point in &profile.points {
                    println!(
                        "  {:>5.1} °C -> {:>5.1}%",
                        point.temperature_c, point.percent
                    );
                }
                println!();
                println!(
                    "{}",
                    ui.info("Dynamic user curve; preview/simulation only on this build.")
                );
            }
            Ok(0)
        }
        CustomProfileCommand::Create {
            name,
            title,
            description,
            points,
            min_percent,
            max_percent,
            hysteresis,
            max_step,
            stale_ms,
            fans,
            file,
        } => {
            let profile = if let Some(file) = file {
                let profile = read_custom_file(file)?;
                if profile.name != *name {
                    return Err(CliError::new(
                        9,
                        format!(
                            "profile file name '{}' does not match --name '{}'",
                            profile.name, name
                        ),
                    ));
                }
                profile
            } else {
                let profile = CustomProfile {
                    name: name.clone(),
                    title: title.clone().unwrap_or_else(|| name.clone()),
                    description: description.clone(),
                    points: parse_points(points)?,
                    min_percent: *min_percent,
                    max_percent: *max_percent,
                    hysteresis_c: *hysteresis,
                    max_step_percent: *max_step,
                    stale_after_ms: *stale_ms,
                    fan_roles: parse_fan_roles(fans)?,
                };
                profile.validate().map_err(|e| CliError::new(9, e))?;
                profile
            };
            let value = submit_custom_create(cli, profile)?;
            print_custom_result(ui, &value)?;
            Ok(0)
        }
        CustomProfileCommand::Update {
            name,
            title,
            description,
            points,
            min_percent,
            max_percent,
            hysteresis,
            max_step,
            stale_ms,
            fans,
            file,
        } => {
            let mut profile = custom_profile(cli, name)?;
            if let Some(file) = file {
                let replacement = read_custom_file(file)?;
                if replacement.name != *name {
                    return Err(CliError::new(
                        9,
                        format!(
                            "profile file name '{}' does not match the profile being updated '{}'",
                            replacement.name, name
                        ),
                    ));
                }
                profile = replacement;
            } else {
                if let Some(title) = title {
                    profile.title = title.clone();
                }
                if let Some(description) = description {
                    profile.description = description.clone();
                }
                if !points.is_empty() {
                    profile.points = parse_points(points)?;
                }
                if let Some(value) = min_percent {
                    profile.min_percent = *value;
                }
                if let Some(value) = max_percent {
                    profile.max_percent = *value;
                }
                if let Some(value) = hysteresis {
                    profile.hysteresis_c = *value;
                }
                if let Some(value) = max_step {
                    profile.max_step_percent = *value;
                }
                if let Some(value) = stale_ms {
                    profile.stale_after_ms = *value;
                }
                if !fans.is_empty() {
                    profile.fan_roles = parse_fan_roles(fans)?;
                }
            }
            profile.validate().map_err(|e| CliError::new(9, e))?;
            let value = submit_custom_update(cli, profile)?;
            print_custom_result(ui, &value)?;
            Ok(0)
        }
        CustomProfileCommand::Duplicate { from, to } => {
            let value = submit_custom_simple(
                cli,
                "custom.duplicate",
                serde_json::json!({"from": from, "to": to}),
                |store| store.duplicate(from, to).map(|_| store.status()),
                "duplicated",
            )?;
            print_custom_result(ui, &value)?;
            Ok(0)
        }
        CustomProfileCommand::Rename { from, to } => {
            let value = submit_custom_simple(
                cli,
                "custom.rename",
                serde_json::json!({"from": from, "to": to}),
                |store| store.rename(from, to).map(|_| store.status()),
                "renamed",
            )?;
            print_custom_result(ui, &value)?;
            Ok(0)
        }
        CustomProfileCommand::Delete { name, yes } => {
            if !yes {
                return Err(CliError::new(
                    8,
                    format!("delete custom profile '{name}' with --yes"),
                ));
            }
            let value = submit_custom_simple(
                cli,
                "custom.delete",
                serde_json::json!({"name": name, "confirm": true}),
                |store| store.delete(name).map(|_| store.status()),
                "deleted",
            )?;
            print_custom_result(ui, &value)?;
            Ok(0)
        }
        CustomProfileCommand::Validate { file } => {
            let profile = read_custom_file(file)?;
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.custom_profile_validation/v1",
                    "valid": true,
                    "name": profile.name,
                }))?;
            } else {
                println!(
                    "{} profile '{}' is valid ({} points, {:.0}–{:.0}%)",
                    ui.ok("OK"),
                    profile.name,
                    profile.points.len(),
                    profile.min_percent,
                    profile.max_percent
                );
            }
            Ok(0)
        }
        CustomProfileCommand::Edit { name } => {
            let profile = custom_profile(cli, name)?;
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .unwrap_or_else(|_| "vi".to_string());
            let temp = editor_path(name);
            let json = serde_json::to_string_pretty(&profile)
                .map_err(|e| CliError::new(1, format!("serialize profile: {e}")))?;
            write_private_file(&temp, &json).map_err(store_cli_error)?;
            let status = std::process::Command::new(&editor)
                .arg(&temp)
                .status()
                .map_err(|e| CliError::new(1, format!("run editor '{editor}': {e}")))?;
            if !status.success() {
                let _ = std::fs::remove_file(&temp);
                return Err(CliError::new(
                    1,
                    format!("editor '{editor}' exited unsuccessfully"),
                ));
            }
            let updated = read_custom_file(&temp);
            let _ = std::fs::remove_file(&temp);
            let updated = updated?;
            if updated.name != *name {
                return Err(CliError::new(
                    9,
                    "renaming during edit is not supported; use `profile custom rename`",
                ));
            }
            updated.validate().map_err(|e| CliError::new(9, e))?;
            let value = submit_custom_update(cli, updated)?;
            print_custom_result(ui, &value)?;
            Ok(0)
        }
        CustomProfileCommand::Path => {
            let status = custom_status(cli);
            let supervised = optional_client(cli).is_some();
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.custom_profile_path/v1",
                    "path": status.path,
                    "supervisor": supervised,
                    "client_path": CustomStore::client().path().display().to_string(),
                }))?;
            } else {
                println!("store        {}", status.path);
                println!(
                    "scope        {}",
                    if supervised {
                        "supervisor store"
                    } else {
                        "unprivileged client store (supervisor unavailable)"
                    }
                );
                println!("client path  {}", CustomStore::client().path().display());
            }
            Ok(0)
        }
        CustomProfileCommand::Reset { yes } => {
            if !yes {
                return Err(CliError::new(
                    8,
                    "resetting the custom profile store requires --yes",
                ));
            }
            if let Some(mut client) = optional_client(cli) {
                match client.call("custom.reset", serde_json::json!({"confirm": true})) {
                    Ok(value) => {
                        print_custom_result(ui, &value)?;
                        return Ok(0);
                    }
                    Err(error) if error.code == "unknown_method" => {}
                    Err(error) => return Err(error.into()),
                }
            }
            let store = CustomStore::client();
            let preserved = store.reset(true).map_err(store_cli_error)?;
            let value = local_custom_result("reset", store.status(), Vec::new());
            print_custom_result(
                ui,
                &serde_json::json!({
                    "schema": value["schema"].clone(),
                    "action": "reset",
                    "status": value["status"].clone(),
                    "preserved": preserved.map(|path| path.display().to_string()),
                }),
            )?;
            Ok(0)
        }
    }
}

fn command_hardware(cli: &Cli, ui: &Ui, command: &HardwareCommand) -> Result<u8, CliError> {
    match command {
        HardwareCommand::List => {
            // Plain configuration files, not an NBFC process. The CLI must not
            // invoke the NBFC client even for read-only discovery.
            let configs = nbfc_config::list_config_names().map_err(|e| CliError::new(7, e))?;
            if ui.json {
                ui.print_json(&serde_json::json!({
                    "schema": "neosense.hardware_list/v1",
                    "source": "configuration_directories",
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
            let view = read_view(&effective_socket(cli));
            let status = view.status.ok_or_else(|| {
                CliError::new(
                    7,
                    view.backend_error
                        .clone()
                        .unwrap_or_else(|| "NBFC status unavailable from the supervisor".into()),
                )
            })?;
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
            let mut client = daemon_client(cli)?;
            let value = client
                .call("hardware.validate", serde_json::json!({"name": name}))
                .map_err(CliError::from)?;
            if ui.json {
                ui.print_json(&value)?;
            } else {
                println!("{}", ui.heading(&format!("Analysis of {name}")));
                match value.get("analysis") {
                    Some(Value::String(text)) => println!("{text}"),
                    Some(analysis) => println!(
                        "{}",
                        serde_json::to_string_pretty(analysis)
                            .map_err(|e| CliError::new(1, format!("JSON serialization: {e}")))?
                    ),
                    None => println!("{}", ui.warn("no analysis returned by the supervisor")),
                }
                println!(
                    "{}",
                    ui.info("Read-only configuration analysis; no fan write performed.")
                );
            }
            Ok(0)
        }
        HardwareCommand::Identity { config } => {
            let view = read_view(&effective_socket(cli));
            let selected = if let Some(name) = config {
                name.clone()
            } else {
                let status = view.status.as_ref().ok_or_else(|| {
                    CliError::new(
                        7,
                        view.backend_error.clone().unwrap_or_else(|| {
                            "NBFC status unavailable from the supervisor".into()
                        }),
                    )
                })?;
                status.selected_config.clone().ok_or_else(|| {
                    CliError::new(7, "NBFC status does not report a selected configuration")
                })?
            };
            // Sensor-source mapping is only authoritative when the requested
            // configuration is the one the supervisor is actually running.
            let sensor_config = view
                .status
                .as_ref()
                .and_then(|status| status.selected_config.as_deref())
                .filter(|selected_config| *selected_config == selected)
                .and(view.sensor_config.as_ref());
            let report = nbfc_config::identity_report(&selected, sensor_config)
                .map_err(|e| CliError::new(7, e))?
                .with_rpm_feedback(&view.snapshot, now_ms(), 3_000);
            if ui.json {
                ui.print_json(&report)?;
            } else {
                println!("{}", ui.heading("NBFC fan identity and capability"));
                println!("config       {}", report.selected_config);
                println!("path         {}", report.config_path);
                println!("model        {}", report.notebook_model);
                if !report.author.is_empty() {
                    println!("author       {}", report.author);
                }
                println!(
                    "critical     {} °C",
                    report
                        .critical_temperature
                        .map(|value| format!("{value:.1}"))
                        .unwrap_or_else(|| "n/a".into())
                );
                for fan in &report.fans {
                    println!(
                        "  fan {} {:<12} role={:<7} {}  readback={}  sensors={}",
                        fan.index,
                        fan.name,
                        fan.role.to_string().to_ascii_lowercase(),
                        fan.register_pair,
                        if fan.readback_configured {
                            "configured"
                        } else {
                            "invalid"
                        },
                        if fan.sensor_sources.is_empty() {
                            "none".to_string()
                        } else {
                            fan.sensor_sources.join(",")
                        }
                    );
                }
                println!();
                for warning in &report.warnings {
                    println!("{} {warning}", ui.warn("warning:"));
                }
                if let Some(error) = &view.sensor_source_error {
                    println!("{} {error}", ui.warn("sensor source:"));
                }
                println!();
                if report.feedback.rpm_channels.is_empty() {
                    println!("{}", ui.info("RPM feedback: unavailable."));
                } else {
                    println!(
                        "{}",
                        ui.heading("Physical RPM feedback (neutral channel names)")
                    );
                    for channel in &report.feedback.rpm_channels {
                        println!(
                            "  {:<14} {:>10}  {:<10} fresh={} feedback={} regulation={}",
                            channel.name,
                            channel
                                .value
                                .map(|value| format!("{value:.0} rpm"))
                                .unwrap_or_else(|| "unavailable".into()),
                            channel.trust.short_label().to_ascii_lowercase(),
                            channel.fresh,
                            channel.usable_for_feedback,
                            channel.usable_for_regulation
                        );
                    }
                    println!(
                        "{}",
                        ui.info(
                            "Acer hwmon channels are physical measurements; CPU/GPU fan identity is not verified."
                        )
                    );
                }
                println!();
                println!(
                    "{}",
                    ui.info(
                        "Read-only identity evidence; physical fan mapping is not proven. RPM feedback is separate from control."
                    )
                );
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
                if let Some(capabilities) = supervisor
                    .status
                    .as_ref()
                    .and_then(|status| status.get("capabilities"))
                {
                    let controlled = capabilities
                        .get("access_controlled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let mode = capabilities
                        .get("socket")
                        .and_then(|socket| socket.get("mode"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    println!(
                        "nbfc access   {} (socket mode {})",
                        if controlled {
                            ui.ok("verified controlled")
                        } else {
                            ui.warn("not access-controlled")
                        },
                        mode
                    );
                } else {
                    println!(
                        "nbfc access   unknown (supervisor did not report access-control capability)"
                    );
                }
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
                    "note": "Safety settings are compiled; persistent custom curves live in the supervisor-owned profile store."
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
                println!(
                    "Safety settings are compiled; custom curves use the supervisor-owned profile store."
                );
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

fn command_calibration(ui: &Ui, command: &CalibrationCommand) -> Result<u8, CliError> {
    match command {
        CalibrationCommand::Analyze { input } => {
            let text = std::fs::read_to_string(input)
                .map_err(|e| CliError::new(2, format!("read {}: {e}", input.display())))?;
            let parsed: CalibrationInput = serde_json::from_str(&text)
                .map_err(|e| CliError::new(2, format!("parse {}: {e}", input.display())))?;
            let report = analyze_calibration(&parsed).map_err(|e| CliError::new(2, e))?;
            if ui.json {
                ui.print_json(&report)?;
            } else {
                println!("{}", ui.heading("Calibration analysis (P1)"));
                println!("  reference          : {}", report.reference_kind.label());
                println!("  samples            : {}", report.sample_count);
                println!(
                    "  instrument         : {}",
                    report.instrument.as_deref().unwrap_or("unspecified")
                );
                println!(
                    "  instrument uncert. : {}",
                    report
                        .instrument_uncertainty
                        .map(|u| u.to_string())
                        .unwrap_or_else(|| "unspecified".into())
                );
                println!(
                    "  absolute RPM result: {}",
                    if report.absolute_rpm_calibration {
                        ui.ok("accepted")
                    } else {
                        ui.warn("not accepted")
                    }
                );
                for channel in &report.channels {
                    println!(
                        "
  {}: {}",
                        channel.channel,
                        if channel.accepted {
                            ui.ok("PASS")
                        } else {
                            ui.warn("FAIL")
                        }
                    );
                    println!(
                        "    slope {:.6}  95% CI [{:.6}, {:.6}]",
                        channel.slope, channel.slope_95_ci[0], channel.slope_95_ci[1]
                    );
                    println!(
                        "    intercept {:.3} RPM  95% CI [{:.3}, {:.3}]",
                        channel.intercept, channel.intercept_95_ci[0], channel.intercept_95_ci[1]
                    );
                    println!(
                        "    R² {:.6}  residual SD {:.3}",
                        channel.r_squared, channel.residual_standard_deviation
                    );
                    for reason in &channel.reasons {
                        println!("    reason: {reason}");
                    }
                }
                for note in &report.notes {
                    println!(
                        "
{} {note}",
                        ui.info("note:")
                    );
                }
            }
            Ok(if report.absolute_rpm_calibration {
                0
            } else {
                6
            })
        }
    }
}

fn command_control(cli: &Cli, ui: &Ui, command: &ControlCommand) -> Result<u8, CliError> {
    match command {
        ControlCommand::Readiness => {
            let view = read_view(&effective_socket(cli));
            let control = neosense::diagnostics::control_readiness(&view);
            if ui.json {
                ui.print_json(&control)?;
            } else {
                print_control_readiness(ui, &control);
            }
            Ok(0)
        }
    }
}

fn print_control_readiness(ui: &Ui, control: &ControlReadiness) {
    println!("{}", ui.heading("Control readiness"));
    println!("  Stage             : {}", control.stage.label());
    println!("  Runtime mode      : {}", control.mode.label());
    println!(
        "  Manual path       : {}",
        if control.manual_path_eligible {
            ui.ok("at least one fan eligible (not physical validation)")
        } else {
            ui.error("blocked")
        }
    );
    println!(
        "  All fans eligible : {}",
        if control.all_fans_eligible {
            ui.ok("yes")
        } else {
            ui.warn("no")
        }
    );
    if !control.eligible_fans.is_empty() || !control.blocked_fans.is_empty() {
        println!(
            "  Eligible fans     : {:?}  ·  blocked: {:?}",
            control.eligible_fans, control.blocked_fans
        );
    }
    println!(
        "  Physical identity : {}",
        if control.physical_identity_verified {
            ui.ok("verified")
        } else {
            ui.warn("unverified")
        }
    );
    println!(
        "  RPM calibration   : {}",
        if control.rpm_calibration_verified {
            ui.ok("verified")
        } else {
            ui.warn("unverified")
        }
    );
    println!(
        "  Physical control  : {}",
        if control.physical_control_validated {
            ui.ok("validated")
        } else {
            ui.warn("not validated")
        }
    );
    if let Some(mode) = control.profile_mode {
        println!("  Profile mode      : {}", mode.label());
    }
    if !control.fans.is_empty() {
        println!("\n  {}", ui.heading("Per-fan readiness"));
        for fan in &control.fans {
            println!(
                "    {:<8} #{} {:<14} sources={} temp={}",
                if fan.policy_eligible {
                    ui.ok("ELIGIBLE")
                } else {
                    ui.error("BLOCKED")
                },
                fan.index,
                fan.name,
                if fan.sources.is_empty() {
                    "none".to_string()
                } else {
                    fan.sources.join(",")
                },
                fan.temperature_status.label()
            );
            for reason in &fan.reasons {
                println!("      - {reason}");
            }
            for missing in &fan.validation_unverified {
                println!("      · {missing}");
            }
        }
    }
    println!();
    for scope in [RequirementScope::Control, RequirementScope::Validation] {
        let requirements: Vec<_> = control.requirements_in(scope).collect();
        if requirements.is_empty() {
            continue;
        }
        println!(
            "  {}",
            ui.heading(if scope == RequirementScope::Control {
                "Control requirements"
            } else {
                "Validation evidence"
            })
        );
        for requirement in requirements {
            let label = match requirement.status {
                RequirementStatus::Satisfied => ui.ok("OK"),
                RequirementStatus::Missing => ui.warn("MISSING"),
                RequirementStatus::Failed => ui.error("FAILED"),
                RequirementStatus::Unverified => ui.warn("UNVERIFIED"),
                RequirementStatus::NotApplicable => ui.info("N/A"),
            };
            println!(
                "    [{label}] {:<24} {}",
                requirement.label, requirement.detail
            );
            println!("             evidence: {}", requirement.evidence);
        }
    }
    if !control.blockers.is_empty() {
        println!("\n  {}", ui.heading("Blockers"));
        for blocker in &control.blockers {
            println!("    - {blocker}");
        }
    }
    println!("\n  {}", ui.info(&control.next_action));
}

fn command_diagnose(cli: &Cli, ui: &Ui) -> Result<u8, CliError> {
    let view = read_view(&effective_socket(cli));
    let report: DiagnosticReport = diagnose(&view);
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
        "  Manual path       : {}",
        if summary.fan_control_available {
            ui.ok("eligible (protected backend)")
        } else {
            ui.error("blocked")
        }
    );
    println!(
        "  RPM available     : {}",
        if summary.physical_feedback_available {
            ui.ok("yes")
        } else {
            ui.warn("no")
        }
    );
    println!(
        "  RPM fresh         : {}",
        if summary.physical_feedback_fresh {
            ui.ok("yes")
        } else {
            ui.warn("no")
        }
    );
    println!(
        "  RPM trusted       : {}",
        if summary.physical_feedback_trusted {
            ui.ok("yes (feedback only)")
        } else {
            ui.warn("no")
        }
    );
    println!(
        "  Fan identity      : {}",
        if summary.physical_identity_verified {
            ui.ok("verified")
        } else {
            ui.warn("logical only (CPU/GPU unverified)")
        }
    );
    println!(
        "  Physical control  : {}",
        if summary.physical_control_validated {
            ui.ok("validated")
        } else {
            ui.warn("not validated")
        }
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

    println!("\n{}", ui.heading("Control readiness"));
    println!("  Stage             : {}", report.control.stage.label());
    println!("  Runtime mode      : {}", report.control.mode.label());
    println!(
        "  Fans eligible     : {:?}  ·  blocked: {:?}",
        report.control.eligible_fans, report.control.blocked_fans
    );
    for fan in report
        .control
        .fans
        .iter()
        .filter(|fan| !fan.policy_eligible)
    {
        for reason in &fan.reasons {
            println!("  BLOCKER #{} {}: {reason}", fan.index, fan.name);
        }
    }
    if !report.control.blockers.is_empty() {
        for blocker in &report.control.blockers {
            println!("  {}", ui.warn(&format!("BLOCKER: {blocker}")));
        }
    }
    println!("  Next safe action  : {}", report.control.next_action);

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
