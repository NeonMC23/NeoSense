//! NBFC backend boundary.
//!
//! The backend is deliberately process-based and shell-free. It supports both
//! the human status format of NBFC 0.5.3 and the JSON format introduced in
//! 0.5.4. Parsing is explicit and tested against fixtures; missing fields stay
//! `None` and are never converted to zero.

use crate::util::{CommandOutput, run_command};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NbfcVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl NbfcVersion {
    pub fn parse(input: &str) -> Option<Self> {
        // `nbfc --version` prints "nbfc 0.5.3".
        let token = input.split_whitespace().last()?;
        let mut parts = token.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    pub fn at_least(self, major: u32, minor: u32, patch: u32) -> bool {
        (self.major, self.minor, self.patch) >= (major, minor, patch)
    }
}

impl std::fmt::Display for NbfcVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcFan {
    pub index: u32,
    pub name: String,
    pub temperature: Option<f64>,
    pub critical_temperature: Option<f64>,
    pub auto_mode: Option<bool>,
    pub critical: Option<bool>,
    pub current_speed: Option<f64>,
    pub target_speed: Option<f64>,
    pub requested_speed: Option<f64>,
    pub speed_steps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcStatus {
    pub version: Option<NbfcVersion>,
    pub read_only: Option<bool>,
    pub selected_config: Option<String>,
    pub fans: Vec<NbfcFan>,
    pub warnings: Vec<String>,
}

impl NbfcStatus {
    pub fn fan(&self, index: u32) -> Option<&NbfcFan> {
        self.fans.iter().find(|f| f.index == index)
    }

    pub fn all_auto(&self) -> bool {
        !self.fans.is_empty() && self.fans.iter().all(|f| f.auto_mode == Some(true))
    }

    pub fn any_critical(&self) -> bool {
        self.fans.iter().any(|f| f.critical == Some(true))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorIdentity {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
    pub verified: bool,
    pub source: String,
}

impl SupervisorIdentity {
    pub fn current() -> Self {
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
        if count < 0 {
            return Self {
                uid,
                gid,
                groups: vec![gid],
                verified: false,
                source: "current process; getgroups failed".to_string(),
            };
        }
        let mut groups = vec![0 as libc::gid_t; count as usize];
        let read = unsafe { libc::getgroups(count, groups.as_mut_ptr()) };
        let mut groups: Vec<u32> = if read < 0 {
            vec![gid]
        } else {
            groups.into_iter().collect()
        };
        if !groups.contains(&gid) {
            groups.push(gid);
        }
        Self {
            uid,
            gid,
            groups,
            verified: read >= 0,
            source: "current process".to_string(),
        }
    }

    /// Best-effort identity for CLI/TUI diagnostics. The configured service is
    /// root; the NeoSense IPC socket owner confirms that in the common case. If
    /// the owner is not root, supplementary groups are not verified and the
    /// socket is classified fail-closed.
    pub fn for_view() -> Self {
        let current = Self::current();
        if current.uid == 0 {
            return current;
        }
        match std::fs::metadata("/run/neosense/neosensed.sock") {
            Ok(metadata) if metadata.uid() == 0 => Self {
                uid: 0,
                gid: 0,
                groups: vec![0],
                verified: true,
                source: "assumed root supervisor from NeoSense IPC socket owner".to_string(),
            },
            Ok(metadata) => Self {
                uid: metadata.uid(),
                gid: metadata.gid(),
                groups: vec![metadata.gid()],
                verified: false,
                source: "NeoSense IPC socket owner; supplementary groups unverified".to_string(),
            },
            Err(_) => current,
        }
    }

    fn is_root(&self) -> bool {
        self.uid == 0
    }
}

#[derive(Debug, Clone, Default)]
struct GroupInfo {
    name: Option<String>,
    member_uids: Vec<u32>,
    known: bool,
    unresolved: bool,
}

#[derive(Debug, Clone)]
struct SocketFacts {
    path: String,
    exists: bool,
    is_socket: bool,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    group: GroupInfo,
    acl_present: bool,
    acl_error: Option<String>,
    metadata_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcSocketSecurity {
    pub path: String,
    pub exists: bool,
    pub is_socket: bool,
    pub mode: Option<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub group_name: Option<String>,
    pub ordinary_group_members: usize,
    pub group_or_world_writable: bool,
    pub acl_present: bool,
    pub supervisor_uid: u32,
    pub supervisor_gid: u32,
    pub supervisor_groups_verified: bool,
    pub identity_source: String,
    pub accessible_to_supervisor: bool,
    /// True only when the effective socket permissions grant access to no
    /// non-root identity other than the verified supervisor, with no POSIX ACL
    /// widening, and the supervisor itself can connect.
    pub access_controlled: bool,
    pub error: Option<String>,
}

pub fn default_socket_path() -> PathBuf {
    std::env::var_os("NEOSENSE_NBFC_SOCKET")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/nbfc_service.socket"))
}

fn local_group_info(gid: u32) -> GroupInfo {
    let group_text = match std::fs::read_to_string("/etc/group") {
        Ok(text) => text,
        Err(_) => return GroupInfo::default(),
    };
    let mut info = GroupInfo::default();
    for line in group_text.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 4 {
            continue;
        }
        if fields[2].parse::<u32>().ok() != Some(gid) {
            continue;
        }
        info.known = true;
        info.name = Some(fields[0].to_string());
        for member in fields[3].split(',').filter(|value| !value.is_empty()) {
            match username_to_uid(member) {
                Some(uid) => info.member_uids.push(uid),
                None => info.unresolved = true,
            }
        }
        break;
    }
    if !info.known {
        return info;
    }
    match std::fs::read_to_string("/etc/passwd") {
        Ok(passwd) => {
            for line in passwd.lines() {
                let fields: Vec<&str> = line.split(':').collect();
                if fields.len() < 4 {
                    continue;
                }
                let Some(uid) = fields[2].parse::<u32>().ok() else {
                    continue;
                };
                if fields[3].parse::<u32>().ok() == Some(gid) {
                    info.member_uids.push(uid);
                }
            }
        }
        Err(_) => info.unresolved = true,
    }
    info.member_uids.sort_unstable();
    info.member_uids.dedup();
    info
}

fn username_to_uid(name: &str) -> Option<u32> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 3 && fields[0] == name {
            fields[2].parse().ok()
        } else {
            None
        }
    })
}

fn posix_acl_present(path: &Path) -> Result<bool, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let cpath = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("invalid socket path: {}", path.display()))?;
    let name = b"system.posix_acl_access\0";
    let result = unsafe {
        libc::getxattr(
            cpath.as_ptr(),
            name.as_ptr() as *const libc::c_char,
            std::ptr::null_mut(),
            0,
        )
    };
    if result >= 0 {
        Ok(result > 0)
    } else {
        let error = std::io::Error::last_os_error();
        let code = error.raw_os_error().unwrap_or_default();
        if code == libc::ENODATA || code == libc::ENOTSUP {
            Ok(false)
        } else {
            Err(format!(
                "cannot inspect POSIX ACL on {}: {error}",
                path.display()
            ))
        }
    }
}

fn classify_socket(facts: SocketFacts, supervisor: &SupervisorIdentity) -> NbfcSocketSecurity {
    let mode = facts.mode;
    let owner_matches = facts.uid == Some(supervisor.uid);
    let owner_write = mode.map(|mode| mode & 0o200 != 0).unwrap_or(false);
    let group_write = mode.map(|mode| mode & 0o020 != 0).unwrap_or(false);
    let world_write = mode.map(|mode| mode & 0o002 != 0).unwrap_or(false);
    let ordinary_group_members = facts
        .group
        .member_uids
        .iter()
        .filter(|uid| **uid != 0 && **uid != supervisor.uid)
        .count();
    let group_allows_supervisor =
        group_write && facts.group.known && facts.group.member_uids.contains(&supervisor.uid);
    let accessible_to_supervisor = if !facts.exists || !facts.is_socket {
        false
    } else if owner_matches && (owner_write || supervisor.is_root()) {
        true
    } else {
        group_allows_supervisor
    };
    let group_safe = !group_write
        || (facts.group.known && !facts.group.unresolved && ordinary_group_members == 0);
    let acl_safe = !facts.acl_present && facts.acl_error.is_none();
    let mut reasons = Vec::new();
    if !facts.exists {
        reasons.push(
            facts
                .metadata_error
                .clone()
                .unwrap_or_else(|| "socket is missing".to_string()),
        );
    } else if !facts.is_socket {
        reasons.push("path exists but is not a Unix socket".to_string());
    }
    if facts.exists && facts.is_socket {
        if !supervisor.verified {
            reasons.push("supervisor identity is unverified".to_string());
        }
        if !owner_matches {
            reasons.push(format!(
                "socket owner uid {:?} does not match supervisor uid {}",
                facts.uid, supervisor.uid
            ));
        }
        if world_write {
            reasons.push("socket is world-writable".to_string());
        }
        if group_write && !world_write && !facts.group.known {
            reasons.push(format!(
                "socket group gid {:?} is not a verifiable local group",
                facts.gid
            ));
        }
        if group_write && !world_write && facts.group.unresolved {
            reasons.push("socket group membership cannot be fully resolved".to_string());
        }
        if group_write && !world_write && ordinary_group_members > 0 {
            reasons.push(format!(
                "socket group grants access to {ordinary_group_members} non-supervisor user(s)"
            ));
        }
        if facts.acl_present {
            reasons.push("socket has a POSIX ACL that may widen access".to_string());
        }
        if let Some(error) = &facts.acl_error {
            reasons.push(error.clone());
        }
        if !accessible_to_supervisor {
            reasons.push("the supervisor itself cannot access the socket".to_string());
        }
    }
    let access_controlled = facts.exists
        && facts.is_socket
        && supervisor.verified
        && owner_matches
        && !world_write
        && group_safe
        && acl_safe
        && accessible_to_supervisor;
    NbfcSocketSecurity {
        path: facts.path,
        exists: facts.exists,
        is_socket: facts.is_socket,
        mode: facts.mode.map(|mode| format!("{mode:03o}")),
        uid: facts.uid,
        gid: facts.gid,
        group_name: facts.group.name,
        ordinary_group_members,
        group_or_world_writable: group_write || world_write,
        acl_present: facts.acl_present,
        supervisor_uid: supervisor.uid,
        supervisor_gid: supervisor.gid,
        supervisor_groups_verified: supervisor.verified,
        identity_source: supervisor.source.clone(),
        accessible_to_supervisor,
        access_controlled,
        error: if access_controlled {
            None
        } else {
            Some(reasons.join("; "))
        },
    }
}

pub fn inspect_socket_with_identity(
    path: &Path,
    supervisor: &SupervisorIdentity,
) -> NbfcSocketSecurity {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let mode = metadata.permissions().mode() & 0o777;
            let gid = metadata.gid();
            let acl = posix_acl_present(path);
            classify_socket(
                SocketFacts {
                    path: path.display().to_string(),
                    exists: true,
                    is_socket: metadata.file_type().is_socket(),
                    mode: Some(mode),
                    uid: Some(metadata.uid()),
                    gid: Some(gid),
                    group: local_group_info(gid),
                    acl_present: acl.as_ref().copied().unwrap_or(false),
                    acl_error: acl.err(),
                    metadata_error: None,
                },
                supervisor,
            )
        }
        Err(error) => classify_socket(
            SocketFacts {
                path: path.display().to_string(),
                exists: false,
                is_socket: false,
                mode: None,
                uid: None,
                gid: None,
                group: GroupInfo::default(),
                acl_present: false,
                acl_error: None,
                metadata_error: Some(format!("socket not available: {error}")),
            },
            supervisor,
        ),
    }
}

pub fn inspect_socket(path: &Path) -> NbfcSocketSecurity {
    inspect_socket_with_identity(path, &SupervisorIdentity::current())
}

pub fn inspect_default_socket() -> NbfcSocketSecurity {
    inspect_socket_with_identity(&default_socket_path(), &SupervisorIdentity::for_view())
}

/// Test-only secure socket used by unit tests that exercise manual/preset
/// policy. Integration tests and production never call this.
#[cfg(test)]
pub fn test_secure_socket_path() -> PathBuf {
    use std::sync::OnceLock;
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = tempfile::tempdir().expect("test socket directory");
        let path = dir.path().join("nbfc-test.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("test socket");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("test socket permissions");
        // Keep the listener and directory alive for the test process.
        std::mem::forget(listener);
        std::mem::forget(dir);
        path
    })
    .clone()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NbfcCapabilities {
    pub version: NbfcVersion,
    pub json_status: bool,
    pub reset_ec: bool,
    pub config_apply: bool,
    pub protected_manual: bool,
    /// This is intentionally conservative: a version-level manual-control
    /// guarantee is not enough while an untrusted local client can bypass the
    /// supervisor through the NBFC socket.
    #[serde(default)]
    pub access_controlled: bool,
    #[serde(default)]
    pub socket: Option<NbfcSocketSecurity>,
}

impl NbfcCapabilities {
    pub fn from_version(version: NbfcVersion) -> Self {
        let json_status = version.at_least(0, 5, 4);
        Self {
            version,
            json_status,
            reset_ec: version.at_least(0, 5, 4),
            config_apply: !version.at_least(0, 5, 4),
            protected_manual: version.at_least(0, 5, 4),
            access_controlled: false,
            socket: None,
        }
    }

    pub fn with_socket_security(mut self, socket: NbfcSocketSecurity) -> Self {
        self.access_controlled = socket.access_controlled;
        self.socket = Some(socket);
        self
    }

    pub fn from_version_with_socket(version: NbfcVersion) -> Self {
        Self::from_version(version).with_socket_security(inspect_default_socket())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NbfcFanSensor {
    pub index: u32,
    pub name: String,
    pub sensors: Vec<String>,
    pub algorithm: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NbfcSensorConfig {
    pub fans: Vec<NbfcFanSensor>,
}

#[derive(Debug, Clone)]
pub struct NbfcCli {
    pub program: String,
    pub timeout: Duration,
}

impl Default for NbfcCli {
    fn default() -> Self {
        Self {
            program: "nbfc".to_string(),
            timeout: Duration::from_secs(5),
        }
    }
}

impl NbfcCli {
    pub fn new(program: impl Into<String>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            timeout,
        }
    }

    fn run(&self, args: &[&str]) -> Result<CommandOutput, String> {
        run_command(&self.program, args, self.timeout).map_err(|e| format!("NBFC unavailable: {e}"))
    }

    pub fn version(&self) -> Result<NbfcVersion, String> {
        let output = self.run(&["--version"])?;
        if !output.success {
            return Err(format!("nbfc --version: {}", output.combined_error()));
        }
        NbfcVersion::parse(&output.stdout)
            .ok_or_else(|| format!("unreadable NBFC version: {:?}", output.stdout.trim()))
    }

    pub fn capabilities(&self) -> Result<NbfcCapabilities, String> {
        self.version()
            .map(NbfcCapabilities::from_version_with_socket)
    }

    pub fn status(&self, version: NbfcVersion) -> Result<NbfcStatus, String> {
        if version.at_least(0, 5, 4) {
            match self.status_json() {
                Ok(status) => return Ok(status),
                Err(json_error) => {
                    // A service can be stopped, or a 0.5.4 JSON schema can be
                    // absent. Try the human format before returning a combined
                    // error, but never hide both failures.
                    return self.status_human().map_err(|human_error| {
                        format!("{json_error}; text format: {human_error}")
                    });
                }
            }
        }
        self.status_human()
    }

    fn status_json(&self) -> Result<NbfcStatus, String> {
        let output = self.run(&["status", "--json"])?;
        if !output.success {
            return Err(format!("nbfc status --json: {}", output.combined_error()));
        }
        parse_status_json(&output.stdout)
    }

    fn status_human(&self) -> Result<NbfcStatus, String> {
        let output = self.run(&["status"])?;
        if !output.success {
            return Err(format!("nbfc status: {}", output.combined_error()));
        }
        parse_status_human(&output.stdout)
    }

    pub fn set_auto(&self) -> Result<CommandOutput, String> {
        self.run(&["set", "-a"])
    }

    pub fn set_speed(&self, fan: u32, percent: f64) -> Result<CommandOutput, String> {
        let args = set_speed_args(fan, percent);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs)
    }

    pub fn list_configs(&self) -> Result<Vec<String>, String> {
        let output = self.run(&["config", "-l"])?;
        if !output.success {
            return Err(format!("nbfc config -l: {}", output.combined_error()));
        }
        Ok(output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub fn sensors_show(&self) -> Result<NbfcSensorConfig, String> {
        let output = self.run(&["sensors", "show"])?;
        if !output.success {
            return Err(format!("nbfc sensors show: {}", output.combined_error()));
        }
        Ok(parse_sensors_show(&output.stdout))
    }

    pub fn rate_config_json(&self, name: &str) -> Result<String, String> {
        let output = run_command(
            &self.program,
            &["rate-config", "-j", name],
            Duration::from_secs(30),
        )
        .map_err(|e| format!("NBFC unavailable: {e}"))?;
        if !output.success {
            return Err(format!("nbfc rate-config: {}", output.combined_error()));
        }
        Ok(output.stdout)
    }

    /// Return to firmware/EC control when the version exposes `reset-ec`.
    /// This is a privileged, version-specific operation and must only be used
    /// by the fail-safe helper, never by ordinary CLI status commands.
    pub fn reset_ec(&self) -> Result<CommandOutput, String> {
        self.run(&["reset-ec"])
    }
}

/// The exact NBFC argument vector for a fixed-speed request. Kept as a pure
/// function so regression tests can assert `-s` is always present.
pub fn set_speed_args(fan: u32, percent: f64) -> Vec<String> {
    vec![
        "set".to_string(),
        "-f".to_string(),
        fan.to_string(),
        "-s".to_string(),
        format!("{percent:.2}"),
    ]
}

/// Narrow interface used by the supervisor. Tests provide a fake backend and
/// must never touch real hardware.
pub trait FanBackend: Send + Sync {
    fn version(&self) -> Result<NbfcVersion, String>;
    fn status(&self, version: NbfcVersion) -> Result<NbfcStatus, String>;
    fn set_auto(&self) -> Result<CommandOutput, String>;
    fn set_speed(&self, fan: u32, percent: f64) -> Result<CommandOutput, String>;
    fn sensors_show(&self) -> Result<NbfcSensorConfig, String>;
    fn list_configs(&self) -> Result<Vec<String>, String>;
}

impl FanBackend for NbfcCli {
    fn version(&self) -> Result<NbfcVersion, String> {
        NbfcCli::version(self)
    }
    fn status(&self, version: NbfcVersion) -> Result<NbfcStatus, String> {
        NbfcCli::status(self, version)
    }
    fn set_auto(&self) -> Result<CommandOutput, String> {
        NbfcCli::set_auto(self)
    }
    fn set_speed(&self, fan: u32, percent: f64) -> Result<CommandOutput, String> {
        NbfcCli::set_speed(self, fan, percent)
    }
    fn sensors_show(&self) -> Result<NbfcSensorConfig, String> {
        NbfcCli::sensors_show(self)
    }
    fn list_configs(&self) -> Result<Vec<String>, String> {
        NbfcCli::list_configs(self)
    }
}

pub fn parse_status_human(text: &str) -> Result<NbfcStatus, String> {
    let mut status = NbfcStatus {
        version: None,
        read_only: None,
        selected_config: None,
        fans: Vec::new(),
        warnings: Vec::new(),
    };

    for (line_no, line) in text.lines().enumerate() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            continue;
        }

        match key {
            "Read-only" => status.read_only = parse_bool(value),
            "Selected Config Name" | "Selected Config Id" | "SelectedConfigId" => {
                if !value.is_empty() {
                    status.selected_config = Some(value.to_string());
                }
            }
            "Fan Display Name" => {
                if value.is_empty() {
                    status
                        .warnings
                        .push(format!("line {}: empty fan name", line_no + 1));
                    continue;
                }
                status.fans.push(NbfcFan {
                    index: status.fans.len() as u32,
                    name: value.to_string(),
                    temperature: None,
                    critical_temperature: None,
                    auto_mode: None,
                    critical: None,
                    current_speed: None,
                    target_speed: None,
                    requested_speed: None,
                    speed_steps: None,
                });
            }
            _ => {
                let Some(fan) = status.fans.last_mut() else {
                    continue;
                };
                match key {
                    "Temperature" => fan.temperature = parse_optional_f64(value),
                    "Critical Temperature" => fan.critical_temperature = parse_optional_f64(value),
                    "Auto Control Enabled" => {
                        fan.auto_mode = parse_bool(value);
                        if fan.auto_mode.is_none() {
                            status.warnings.push(format!(
                                "line {}: unreadable Auto Control Enabled boolean: {value:?}",
                                line_no + 1
                            ));
                        }
                    }
                    "Critical Mode Enabled" => {
                        fan.critical = parse_bool(value);
                    }
                    "Current Fan Speed" => fan.current_speed = parse_optional_f64(value),
                    "Target Fan Speed" => fan.target_speed = parse_optional_f64(value),
                    "Requested Fan Speed" => fan.requested_speed = parse_optional_f64(value),
                    "Fan Speed Steps" => {
                        fan.speed_steps = value.parse::<u32>().ok();
                    }
                    _ => {}
                }
            }
        }
    }

    // A status with no fan blocks is valid when the service has no selected
    // config, but the caller must not invent fan data.
    Ok(status)
}

pub fn parse_status_json(text: &str) -> Result<NbfcStatus, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid NBFC JSON: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "NBFC JSON: object expected".to_string())?;

    let read_only = object.get("ReadOnly").and_then(|v| v.as_bool());
    let selected_config = object
        .get("SelectedConfigId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let fans_value = object
        .get("Fans")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "NBFC JSON: missing Fans array".to_string())?;

    let mut fans = Vec::with_capacity(fans_value.len());
    let mut warnings = Vec::new();
    for (index, value) in fans_value.iter().enumerate() {
        let Some(fan) = value.as_object() else {
            warnings.push(format!("Fans[{index}]: object expected"));
            continue;
        };
        let name = fan
            .get("Name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            warnings.push(format!("Fans[{index}]: missing name"));
        }
        fans.push(NbfcFan {
            index: index as u32,
            name,
            temperature: fan.get("Temperature").and_then(|v| v.as_f64()),
            critical_temperature: fan.get("CriticalTemperature").and_then(|v| v.as_f64()),
            auto_mode: fan.get("AutoMode").and_then(|v| v.as_bool()),
            critical: fan.get("Critical").and_then(|v| v.as_bool()),
            current_speed: fan.get("CurrentSpeed").and_then(|v| v.as_f64()),
            target_speed: fan.get("TargetSpeed").and_then(|v| v.as_f64()),
            requested_speed: fan.get("RequestedSpeed").and_then(|v| v.as_f64()),
            speed_steps: fan
                .get("SpeedSteps")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
        });
    }

    Ok(NbfcStatus {
        version: None,
        read_only,
        selected_config,
        fans,
        warnings,
    })
}

pub fn parse_sensors_show(text: &str) -> NbfcSensorConfig {
    let mut config = NbfcSensorConfig::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let Some(open) = trimmed.find('(') else {
                continue;
            };
            let Some(relative_close) = trimmed[open..].find(')') else {
                continue;
            };
            let close = open + relative_close;
            let index = trimmed[1..open]
                .trim()
                .parse::<u32>()
                .unwrap_or(config.fans.len() as u32);
            let name = trimmed[open + 1..close].trim().to_string();
            config.fans.push(NbfcFanSensor {
                index,
                name,
                sensors: Vec::new(),
                algorithm: None,
            });
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Sensors:") {
            if let Some(fan) = config.fans.last_mut() {
                fan.sensors = value
                    .replace("(default)", "")
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        } else if let Some(value) = trimmed.strip_prefix("Algorithm:") {
            if let Some(fan) = config.fans.last_mut() {
                fan.algorithm = Some(value.trim().to_string());
            }
        }
    }
    config
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_optional_f64(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUMAN: &str = "Read-only                : false\n\
Selected Config Name     : Acer Nitro AN515-57 Custom\n\
\n\
Fan Display Name         : CPU fan\n\
Temperature              : 49.88\n\
Auto Control Enabled     : true\n\
Critical Mode Enabled    : false\n\
Current Fan Speed        : 100.00\n\
Target Fan Speed         : 35.00\n\
Fan Speed Steps          : 100\n\
\n\
Fan Display Name         : GPU fan\n\
Temperature              : 49.88\n\
Auto Control Enabled     : false\n\
Critical Mode Enabled    : false\n\
Current Fan Speed        : 100.00\n\
Target Fan Speed         : 40.00\n\
Fan Speed Steps          : 100\n";

    #[test]
    fn parses_real_human_output_and_lowercase_false() {
        let status = parse_status_human(HUMAN).unwrap();
        assert_eq!(status.fans.len(), 2);
        assert_eq!(status.fans[0].name, "CPU fan");
        assert_eq!(status.fans[0].auto_mode, Some(true));
        assert_eq!(status.fans[1].auto_mode, Some(false));
        assert_eq!(status.fans[1].target_speed, Some(40.0));
        assert!(!status.all_auto());
    }

    #[test]
    fn parses_json_format() {
        let json = r#"{
          "PID": 42,
          "SelectedConfigId": "Acer Nitro AN515-57 Custom",
          "ReadOnly": false,
          "Fans": [
            {"Name":"CPU fan","Temperature":58.0,"CriticalTemperature":95.0,
             "AutoMode":true,"Critical":false,"CurrentSpeed":35.0,
             "TargetSpeed":35.0,"RequestedSpeed":-1.0,"SpeedSteps":100}
          ]
        }"#;
        let status = parse_status_json(json).unwrap();
        assert_eq!(
            status.selected_config.as_deref(),
            Some("Acer Nitro AN515-57 Custom")
        );
        assert_eq!(status.fans.len(), 1);
        assert_eq!(status.fans[0].auto_mode, Some(true));
        assert_eq!(status.fans[0].requested_speed, Some(-1.0));
    }

    #[test]
    fn missing_fields_are_none_not_zero() {
        let status = parse_status_human("Fan Display Name : CPU fan\n").unwrap();
        assert_eq!(status.fans.len(), 1);
        assert_eq!(status.fans[0].temperature, None);
        assert_eq!(status.fans[0].target_speed, None);
        assert_eq!(status.fans[0].auto_mode, None);
    }

    #[test]
    fn empty_status_does_not_fabricate_fans() {
        let status = parse_status_human("Read-only : false\n").unwrap();
        assert!(status.fans.is_empty());
        assert!(!status.all_auto());
    }

    #[test]
    fn sensors_show_maps_source_per_fan() {
        let cfg = parse_sensors_show(
            "#0 (CPU fan):\n  Sensors:   @CPU (default)\n  Algorithm: Average (default)\n\
             #1 (GPU fan):\n  Sensors:   @CPU (default)\n  Algorithm: Average (default)\n",
        );
        assert_eq!(cfg.fans.len(), 2);
        assert_eq!(cfg.fans[1].sensors, vec!["@CPU"]);
    }

    #[test]
    fn set_speed_always_contains_speed_option() {
        let args = set_speed_args(0, 50.0);
        assert_eq!(args, vec!["set", "-f", "0", "-s", "50.00"]);
        assert!(args.iter().any(|a| a == "-s"));
    }

    fn identity(uid: u32) -> SupervisorIdentity {
        SupervisorIdentity {
            uid,
            gid: uid,
            groups: vec![uid],
            verified: true,
            source: "test".to_string(),
        }
    }

    fn group(name: &str, members: &[u32]) -> GroupInfo {
        GroupInfo {
            name: Some(name.to_string()),
            member_uids: members.to_vec(),
            known: true,
            unresolved: false,
        }
    }

    fn facts(mode: u32, uid: u32, gid: u32, members: &[u32]) -> SocketFacts {
        SocketFacts {
            path: "/tmp/test.sock".to_string(),
            exists: true,
            is_socket: true,
            mode: Some(mode),
            uid: Some(uid),
            gid: Some(gid),
            group: group("test", members),
            acl_present: false,
            acl_error: None,
            metadata_error: None,
        }
    }

    #[test]
    fn root_owned_0600_is_suitable_for_a_root_supervisor() {
        let security = classify_socket(facts(0o600, 0, 0, &[0]), &identity(0));
        assert!(security.access_controlled);
        assert!(security.accessible_to_supervisor);
        assert_eq!(security.error, None);
    }

    #[test]
    fn root_owned_0600_is_not_suitable_for_a_non_root_supervisor() {
        let security = classify_socket(facts(0o600, 0, 0, &[0]), &identity(1000));
        assert!(!security.access_controlled);
        assert!(!security.accessible_to_supervisor);
        assert!(security.error.unwrap().contains("owner"));
    }

    #[test]
    fn group_writable_socket_with_ordinary_members_is_insecure() {
        let security = classify_socket(facts(0o660, 0, 1000, &[0, 1000]), &identity(0));
        assert!(!security.access_controlled);
        assert_eq!(security.ordinary_group_members, 1);
        assert!(security.error.unwrap().contains("non-supervisor"));
    }

    #[test]
    fn dedicated_group_with_only_the_supervisor_is_accepted() {
        let security = classify_socket(facts(0o660, 0, 4242, &[0]), &identity(0));
        assert!(security.access_controlled);
        let security = classify_socket(facts(0o660, 1000, 4242, &[1000]), &identity(1000));
        assert!(security.access_controlled);
    }

    #[test]
    fn unexpected_group_or_unresolved_membership_fails_closed() {
        let security = classify_socket(facts(0o660, 0, 4242, &[0, 1001]), &identity(0));
        assert!(!security.access_controlled);
        let mut unresolved = facts(0o660, 0, 4242, &[0]);
        unresolved.group.unresolved = true;
        assert!(!classify_socket(unresolved, &identity(0)).access_controlled);
        let mut unknown = facts(0o660, 0, 4242, &[0]);
        unknown.group.known = false;
        assert!(!classify_socket(unknown, &identity(0)).access_controlled);
    }

    #[test]
    fn world_writable_acl_or_wrong_owner_fails_closed() {
        assert!(!classify_socket(facts(0o666, 0, 0, &[0]), &identity(0)).access_controlled);
        let mut acl = facts(0o600, 0, 0, &[0]);
        acl.acl_present = true;
        assert!(!classify_socket(acl, &identity(0)).access_controlled);
        assert!(
            !classify_socket(facts(0o600, 1000, 1000, &[1000]), &identity(0)).access_controlled
        );
    }

    #[test]
    fn unverified_supervisor_identity_fails_closed() {
        let mut supervisor = identity(0);
        supervisor.verified = false;
        assert!(!classify_socket(facts(0o600, 0, 0, &[0]), &supervisor).access_controlled);
    }

    #[test]
    fn actual_socket_mode_change_is_reclassified() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nbfc.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let supervisor = SupervisorIdentity::current();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let secure = inspect_socket_with_identity(&path, &supervisor);
        assert!(secure.access_controlled);
        assert_eq!(secure.mode.as_deref(), Some("600"));
        assert!(secure.uid.is_some());
        assert!(secure.gid.is_some());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let world = inspect_socket_with_identity(&path, &supervisor);
        assert!(!world.access_controlled);
        assert!(world.group_or_world_writable);
    }

    #[test]
    fn replaced_socket_is_reclassified() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nbfc.sock");
        let supervisor = SupervisorIdentity::current();
        let first = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(inspect_socket_with_identity(&path, &supervisor).access_controlled);
        drop(first);
        std::fs::remove_file(&path).unwrap();
        let second = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let security = inspect_socket_with_identity(&path, &supervisor);
        assert!(!security.access_controlled);
        assert_eq!(security.mode.as_deref(), Some("666"));
        drop(second);
    }

    #[test]
    fn missing_nbfc_socket_is_not_access_controlled() {
        let dir = tempfile::tempdir().unwrap();
        let security = inspect_socket(&dir.path().join("missing.sock"));
        assert!(!security.exists);
        assert!(!security.access_controlled);
        assert!(security.error.is_some());
    }

    #[test]
    fn regular_file_is_not_treated_as_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-socket");
        std::fs::write(&path, b"x").unwrap();
        let security = inspect_socket(&path);
        assert!(security.exists);
        assert!(!security.is_socket);
        assert!(!security.access_controlled);
        assert!(security.error.is_some());
    }

    #[test]
    fn capability_enrichment_records_socket_security() {
        use std::os::unix::net::UnixListener;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nbfc.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let caps = NbfcCapabilities::from_version_with_socket(NbfcVersion {
            major: 0,
            minor: 5,
            patch: 4,
        });
        // from_version_with_socket uses the default host path; exercise the
        // explicit enrichment path with the temporary socket instead.
        let caps = caps.with_socket_security(inspect_socket(&path));
        assert!(!caps.access_controlled);
        assert_eq!(caps.socket.as_ref().unwrap().mode.as_deref(), Some("666"));
    }

    #[test]
    fn version_parses() {
        assert_eq!(
            NbfcVersion::parse("nbfc 0.5.3").unwrap(),
            NbfcVersion {
                major: 0,
                minor: 5,
                patch: 3
            }
        );
        assert!(
            NbfcCapabilities::from_version(NbfcVersion::parse("nbfc 0.5.4").unwrap())
                .protected_manual
        );
        assert!(
            !NbfcCapabilities::from_version(NbfcVersion::parse("nbfc 0.5.3").unwrap())
                .protected_manual
        );
    }
}
