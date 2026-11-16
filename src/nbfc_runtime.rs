//! Read-only identification of the NBFC embedded-controller backend.
//!
//! NBFC 0.5.4 does not expose its selected `EmbeddedControllerType` in the
//! `status` protocol. When neither `-e/--embedded-controller` nor the service
//! configuration selects one, NBFC probes `ec_sys`, `acpi_ec` and `dev_port` in
//! that order. This module inspects the running service's command line, its
//! configured value and its open file descriptors. It never opens an EC
//! endpoint, calls `modprobe`, writes hardware or changes NBFC state.
//!
//! A file descriptor is stronger evidence than the configured value because it
//! is what the running process actually opened. If the descriptor directory is
//! not readable, the result is explicitly unknown rather than inferred from
//! module presence or probe order.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "neosense.nbfc_runtime/v1";
pub const DEFAULT_SERVICE_CONFIG: &str = "/etc/nbfc/nbfc.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EcBackend {
    EcSys,
    AcpiEc,
    DevPort,
    Dummy,
}

impl EcBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EcSys => "ec_sys",
            Self::AcpiEc => "acpi_ec",
            Self::DevPort => "dev_port",
            Self::Dummy => "dummy",
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ec_sys" | "ec_sys_linux" => Some(Self::EcSys),
            "acpi_ec" | "acpi_ec_linux" => Some(Self::AcpiEc),
            "dev_port" | "dev_port_linux" | "ec_linux" => Some(Self::DevPort),
            "dummy" => Some(Self::Dummy),
            _ => None,
        }
    }
}

impl std::fmt::Display for EcBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendConfidence {
    /// An open process descriptor identifies the endpoint the service uses.
    Verified,
    /// The service command line or configuration names a backend, but its
    /// open descriptor could not be observed.
    Configured,
    /// Neither source identified the running backend.
    Unknown,
}

impl BackendConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Configured => "configured",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEndpoint {
    pub fd: u32,
    pub path: String,
    pub backend: EcBackend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NbfcRuntimeInfo {
    pub schema: String,
    pub pid: Option<u32>,
    pub command_line: Vec<String>,
    pub config_path: Option<String>,
    /// Raw configured value from `-e/--embedded-controller` or the service
    /// configuration. Kept verbatim so an unknown value is not silently mapped.
    pub configured_backend: Option<String>,
    pub observed_backend: Option<EcBackend>,
    pub effective_backend: Option<EcBackend>,
    pub confidence: BackendConfidence,
    pub endpoints: Vec<RuntimeEndpoint>,
    pub evidence: Vec<String>,
    pub probe_errors: Vec<String>,
}

impl NbfcRuntimeInfo {
    fn unknown(pid: Option<u32>) -> Self {
        Self {
            schema: SCHEMA.to_string(),
            pid,
            command_line: Vec::new(),
            config_path: None,
            configured_backend: None,
            observed_backend: None,
            effective_backend: None,
            confidence: BackendConfidence::Unknown,
            endpoints: Vec::new(),
            evidence: Vec::new(),
            probe_errors: Vec::new(),
        }
    }

    /// One-line description suitable for human output.
    pub fn summary(&self) -> String {
        match (self.effective_backend, self.confidence) {
            (Some(backend), confidence) => {
                format!("{backend} ({})", confidence.label())
            }
            (None, _) => "unknown".to_string(),
        }
    }
}

/// Probe the host's NBFC service. This is read-only and never opens an EC
/// endpoint.
pub fn probe(pid: Option<u32>) -> NbfcRuntimeInfo {
    probe_in(pid, Path::new("/proc"), Path::new(DEFAULT_SERVICE_CONFIG))
}

/// Testable probe with explicit `/proc` and service-config paths.
pub fn probe_in(pid: Option<u32>, proc_root: &Path, default_config: &Path) -> NbfcRuntimeInfo {
    let mut info = NbfcRuntimeInfo::unknown(pid);
    let Some(pid) = pid else {
        info.probe_errors
            .push("NBFC service PID is unavailable".to_string());
        return info;
    };

    let process = proc_root.join(pid.to_string());
    let mut config_path = default_config.to_path_buf();
    match read_command_line(&process.join("cmdline")) {
        Ok(args) => {
            info.command_line = args;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            info.probe_errors
                .push(format!("process {pid} is not present in /proc"));
        }
        Err(error) => info
            .probe_errors
            .push(format!("cannot read process {pid} command line: {error}")),
    }

    if let Some(path) = option_value(&info.command_line, "-c", "--config-file") {
        config_path = PathBuf::from(path);
    }
    info.config_path = Some(config_path.display().to_string());

    if let Some(value) = option_value(&info.command_line, "-e", "--embedded-controller") {
        info.configured_backend = Some(value.clone());
        info.evidence.push(format!(
            "service command line selects --embedded-controller {value}"
        ));
    }
    match read_configured_backend(&config_path) {
        Ok(Some(value)) => {
            info.evidence.push(format!(
                "service configuration {} sets EmbeddedControllerType={value}",
                config_path.display()
            ));
            if info.configured_backend.is_none() {
                info.configured_backend = Some(value);
            }
        }
        Ok(None) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => info.probe_errors.push(format!(
            "cannot inspect service configuration {}: {error}",
            config_path.display()
        )),
    }

    match read_endpoints(&process.join("fd"), pid) {
        Ok(endpoints) => {
            info.endpoints = endpoints;
            for endpoint in &info.endpoints {
                info.evidence.push(format!(
                    "NBFC process {pid} has fd {} open on {} ({})",
                    endpoint.fd, endpoint.path, endpoint.backend
                ));
            }
        }
        Err(error) => info.probe_errors.push(format!(
            "cannot inspect open descriptors for NBFC process {pid}: {error}"
        )),
    }

    let mut observed: Vec<EcBackend> = info
        .endpoints
        .iter()
        .map(|endpoint| endpoint.backend)
        .collect();
    let unique: HashSet<EcBackend> = observed.drain(..).collect();
    let mut unique: Vec<EcBackend> = unique.into_iter().collect();
    unique.sort_by_key(|backend| backend.as_str());
    if unique.len() == 1 {
        info.observed_backend = Some(unique[0]);
    } else if unique.len() > 1 {
        info.probe_errors.push(format!(
            "multiple EC endpoints are open ({}); the effective backend is ambiguous",
            unique
                .iter()
                .map(|backend| backend.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let configured = info
        .configured_backend
        .as_deref()
        .and_then(EcBackend::from_name);
    if let (Some(observed), Some(configured)) = (info.observed_backend, configured)
        && observed != configured
    {
        info.probe_errors.push(format!(
            "configured backend {configured} differs from the observed open endpoint {observed}"
        ));
    }

    // The running descriptor is authoritative. A configured value is retained
    // as weaker evidence when the descriptor could not be observed.
    info.effective_backend = info.observed_backend.or(configured);
    info.confidence = if info.observed_backend.is_some() {
        BackendConfidence::Verified
    } else if info.effective_backend.is_some() {
        BackendConfidence::Configured
    } else {
        BackendConfidence::Unknown
    };
    if info.confidence == BackendConfidence::Unknown {
        info.evidence.push(
            "NBFC status does not expose EmbeddedControllerType; no process descriptor or configured endpoint identified the backend"
                .to_string(),
        );
    }
    info
}

fn read_command_line(path: &Path) -> io::Result<Vec<String>> {
    let bytes = std::fs::read(path)?;
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect())
}

fn read_configured_backend(path: &Path) -> io::Result<Option<String>> {
    let text = std::fs::read_to_string(path)?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(value
        .get("EmbeddedControllerType")
        .and_then(|value| value.as_str())
        .map(str::to_string))
}

fn read_endpoints(path: &Path, _pid: u32) -> io::Result<Vec<RuntimeEndpoint>> {
    let mut endpoints = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let Ok(fd) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        if let Some(backend) = endpoint_backend(&target) {
            endpoints.push(RuntimeEndpoint {
                fd,
                path: target.display().to_string(),
                backend,
            });
        }
    }
    endpoints.sort_by_key(|endpoint| endpoint.fd);
    Ok(endpoints)
}

fn endpoint_backend(path: &Path) -> Option<EcBackend> {
    let path = path.to_string_lossy();
    let path = path.strip_suffix(" (deleted)").unwrap_or(&path);
    match path {
        "/sys/kernel/debug/ec/ec0/io" => Some(EcBackend::EcSys),
        "/dev/ec" => Some(EcBackend::AcpiEc),
        "/dev/port" => Some(EcBackend::DevPort),
        _ => None,
    }
}

fn option_value(args: &[String], short: &str, long: &str) -> Option<String> {
    let long_prefix = format!("{long}=");
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == short || arg == long {
            return args.get(index + 1).cloned();
        }
        if let Some(value) = arg.strip_prefix(&long_prefix) {
            return Some(value.to_string());
        }
        if arg.starts_with(short) && arg.len() > short.len() {
            return Some(arg[short.len()..].to_string());
        }
        index += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_proc(pid: u32, cmdline: &[&str], descriptors: &[(u32, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let process = dir.path().join(pid.to_string());
        std::fs::create_dir_all(process.join("fd")).unwrap();
        let mut bytes = Vec::new();
        for argument in cmdline {
            bytes.extend_from_slice(argument.as_bytes());
            bytes.push(0);
        }
        std::fs::write(process.join("cmdline"), bytes).unwrap();
        for (fd, target) in descriptors {
            std::os::unix::fs::symlink(target, process.join("fd").join(fd.to_string())).unwrap();
        }
        dir
    }

    #[test]
    fn open_ec_sys_descriptor_is_verified_evidence() {
        let proc = fake_proc(
            2393,
            &["nbfc_service", "-f"],
            &[(5, "/sys/kernel/debug/ec/ec0/io")],
        );
        let config = proc.path().join("nbfc.json");
        let info = probe_in(Some(2393), proc.path(), &config);
        assert_eq!(info.observed_backend, Some(EcBackend::EcSys));
        assert_eq!(info.effective_backend, Some(EcBackend::EcSys));
        assert_eq!(info.confidence, BackendConfidence::Verified);
        assert_eq!(info.endpoints.len(), 1);
        assert_eq!(info.endpoints[0].fd, 5);
        assert!(info.evidence.iter().any(|line| line.contains("fd 5")));
        assert!(info.probe_errors.is_empty(), "{:?}", info.probe_errors);
    }

    #[test]
    fn command_line_backend_is_configured_not_verified_without_a_descriptor() {
        let proc = fake_proc(41, &["nbfc_service", "-f", "-e", "acpi_ec"], &[]);
        let config = proc.path().join("nbfc.json");
        let info = probe_in(Some(41), proc.path(), &config);
        assert_eq!(info.observed_backend, None);
        assert_eq!(info.configured_backend.as_deref(), Some("acpi_ec"));
        assert_eq!(info.effective_backend, Some(EcBackend::AcpiEc));
        assert_eq!(info.confidence, BackendConfidence::Configured);
        assert!(info.probe_errors.is_empty(), "{:?}", info.probe_errors);
    }

    #[test]
    fn service_config_can_select_the_backend_and_the_long_option_is_parsed() {
        let proc = fake_proc(
            42,
            &["nbfc_service", "-f", "--embedded-controller=dev_port"],
            &[],
        );
        let config = proc.path().join("nbfc.json");
        std::fs::write(
            &config,
            serde_json::json!({"EmbeddedControllerType": "acpi_ec"}).to_string(),
        )
        .unwrap();
        let info = probe_in(Some(42), proc.path(), &config);
        assert_eq!(info.configured_backend.as_deref(), Some("dev_port"));
        assert_eq!(info.effective_backend, Some(EcBackend::DevPort));
        assert_eq!(info.confidence, BackendConfidence::Configured);
        assert!(
            info.evidence
                .iter()
                .any(|line| line.contains("EmbeddedControllerType=acpi_ec"))
        );
    }

    #[test]
    fn configured_backend_is_used_when_the_descriptor_directory_is_unavailable() {
        let proc = fake_proc(43, &["nbfc_service", "-f"], &[]);
        let config = proc.path().join("nbfc.json");
        std::fs::write(
            &config,
            serde_json::json!({"EmbeddedControllerType": "acpi_ec"}).to_string(),
        )
        .unwrap();
        // Remove the fd directory so the probe must fall back to configuration.
        std::fs::remove_dir(proc.path().join("43").join("fd")).unwrap();
        let info = probe_in(Some(43), proc.path(), &config);
        assert_eq!(info.observed_backend, None);
        assert_eq!(info.effective_backend, Some(EcBackend::AcpiEc));
        assert_eq!(info.confidence, BackendConfidence::Configured);
        assert!(
            info.probe_errors
                .iter()
                .any(|line| line.contains("descriptors"))
        );
    }

    #[test]
    fn conflicting_configured_and_observed_backends_are_reported() {
        let proc = fake_proc(
            44,
            &["nbfc_service", "-f", "-e", "acpi_ec"],
            &[(7, "/sys/kernel/debug/ec/ec0/io")],
        );
        let config = proc.path().join("nbfc.json");
        let info = probe_in(Some(44), proc.path(), &config);
        assert_eq!(info.observed_backend, Some(EcBackend::EcSys));
        assert_eq!(info.effective_backend, Some(EcBackend::EcSys));
        assert_eq!(info.confidence, BackendConfidence::Verified);
        assert!(
            info.probe_errors
                .iter()
                .any(|line| line.contains("differs from the observed"))
        );
    }

    #[test]
    fn no_pid_and_unknown_process_are_honest_unknowns() {
        let info = probe_in(
            None,
            Path::new("/nonexistent"),
            Path::new("/nonexistent.json"),
        );
        assert_eq!(info.confidence, BackendConfidence::Unknown);
        assert!(info.probe_errors.iter().any(|line| line.contains("PID")));

        let proc = fake_proc(45, &["nbfc_service", "-f"], &[]);
        let info = probe_in(Some(99), proc.path(), &proc.path().join("nbfc.json"));
        assert_eq!(info.confidence, BackendConfidence::Unknown);
        assert!(
            info.probe_errors
                .iter()
                .any(|line| line.contains("not present"))
        );
    }

    #[test]
    fn ec_backend_names_are_stable() {
        assert_eq!(EcBackend::from_name("ec_sys"), Some(EcBackend::EcSys));
        assert_eq!(EcBackend::from_name("ec_sys_linux"), Some(EcBackend::EcSys));
        assert_eq!(EcBackend::from_name("acpi_ec"), Some(EcBackend::AcpiEc));
        assert_eq!(EcBackend::from_name("dev_port"), Some(EcBackend::DevPort));
        assert_eq!(EcBackend::from_name("unknown"), None);
        assert_eq!(EcBackend::EcSys.to_string(), "ec_sys");
    }
}
