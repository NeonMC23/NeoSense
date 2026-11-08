use clap::Parser;
use neosense::daemon::{Daemon, DaemonConfig, fail_safe, socket_path_from_env};
use neosense::nbfc::NbfcCli;
use neosense::sensors::SysfsRoot;
use signal_hook::consts::{SIGINT, SIGTERM};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[derive(Parser, Debug)]
#[command(
    name = "neosensed",
    version = neosense::VERSION,
    about = "NeoSense thermal supervisor (sole fan-write authority)"
)]
struct Args {
    /// Supervisor Unix socket.
    #[arg(long, env = "NEOSENSE_SOCKET")]
    socket: Option<PathBuf>,
    /// Sampling interval in milliseconds.
    #[arg(long, default_value_t = 2000)]
    interval: u64,
    /// Do not force automatic mode at startup if NBFC is in fixed mode.
    #[arg(long)]
    no_startup_safe_auto: bool,
    /// Fail-safe mode for systemd ExecStopPost: return to automatic, verify, then exit.
    #[arg(long)]
    fail_safe: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let backend = Arc::new(NbfcCli::default());

    if args.fail_safe {
        return match fail_safe(backend.as_ref()) {
            Ok(message) => {
                println!("neosensed: {message}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("neosensed: fail-safe failed: {error}");
                ExitCode::FAILURE
            }
        };
    }

    let config = DaemonConfig {
        socket: args.socket.clone().unwrap_or_else(socket_path_from_env),
        interval_ms: args.interval.max(500),
        startup_safe_auto: !args.no_startup_safe_auto,
        ..DaemonConfig::default()
    };

    let daemon = Arc::new(Daemon::new(backend, SysfsRoot::default(), config));
    if let Err(error) = daemon.enforce_startup_safety() {
        eprintln!("neosensed: safe startup state impossible: {error}");
        return ExitCode::FAILURE;
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    if let Err(error) = signal_hook::flag::register(SIGTERM, Arc::clone(&shutdown)) {
        eprintln!("neosensed: SIGTERM: {error}");
        return ExitCode::FAILURE;
    }
    if let Err(error) = signal_hook::flag::register(SIGINT, Arc::clone(&shutdown)) {
        eprintln!("neosensed: SIGINT: {error}");
        return ExitCode::FAILURE;
    }

    match daemon.run(shutdown) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("neosensed: {error}");
            ExitCode::FAILURE
        }
    }
}
