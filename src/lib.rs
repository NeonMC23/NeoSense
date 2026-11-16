//! NeoSense core library.
//!
//! The library is shared by the CLI and the supervisor. It contains no direct
//! fan writes; all writes are mediated by `daemon::Daemon` through a narrow
//! `FanBackend` implementation.

pub mod calibration;
pub mod calibration_readiness;
pub mod control;
pub mod custom_profiles;
pub mod daemon;
pub mod diagnostics;
pub mod fans;
pub mod ipc;
pub mod model;
pub mod nbfc;
pub mod nbfc_config;
pub mod nbfc_runtime;
pub mod policy;
pub mod presets;
pub mod profiles;
pub mod regulation;
pub mod runtime;
pub mod sensors;
pub mod service;
pub mod supervisor;
pub mod systemd;
pub mod tui;
pub mod util;
pub mod validation;
pub mod warnings;

pub use model::VERSION;
