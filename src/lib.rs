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
pub mod sensors;
pub mod service;
pub mod util;

pub use model::VERSION;
