//! NeoSense core library.
//!
//! The library is shared by the CLI and the supervisor. It contains no direct
//! fan writes; all writes are mediated by `daemon::Daemon` through a narrow
//! `FanBackend` implementation.

pub mod daemon;
pub mod diagnostics;
pub mod ipc;
pub mod model;
pub mod nbfc;
pub mod policy;
pub mod profiles;
pub mod regulation;
pub mod runtime;
pub mod sensors;
pub mod service;
pub mod supervisor;
pub mod systemd;
pub mod tui;
pub mod util;
pub mod warnings;

pub use model::VERSION;
