use crate::util::run_command;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub active_state: Option<String>,
    pub sub_state: Option<String>,
    pub unit_file_state: Option<String>,
    pub load_state: Option<String>,
    pub main_pid: Option<u32>,
    pub error: Option<String>,
}

impl ServiceStatus {
    pub fn is_active(&self) -> bool {
        self.active_state.as_deref() == Some("active")
    }

    pub fn is_enabled(&self) -> bool {
        matches!(
            self.unit_file_state.as_deref(),
            Some("enabled" | "enabled-runtime" | "static" | "indirect" | "generated")
        )
    }
}

pub fn nbfc_service_status() -> ServiceStatus {
    match systemctl_show("nbfc_service") {
        Ok(s) => s,
        Err(e) => ServiceStatus {
            error: Some(e),
            ..ServiceStatus::default()
        },
    }
}

pub fn systemctl_show(unit: &str) -> Result<ServiceStatus, String> {
    let output = run_command(
        "systemctl",
        &[
            "show",
            unit,
            "-p",
            "ActiveState,SubState,UnitFileState,LoadState,MainPID",
        ],
        Duration::from_secs(3),
    )?;
    if !output.success {
        return Err(format!(
            "systemctl show {unit}: {}",
            output.combined_error()
        ));
    }
    Ok(parse_show(&output.stdout))
}

pub fn parse_show(text: &str) -> ServiceStatus {
    let mut status = ServiceStatus::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "ActiveState" => status.active_state = Some(value.to_string()),
            "SubState" => status.sub_state = Some(value.to_string()),
            "UnitFileState" => status.unit_file_state = Some(value.to_string()),
            "LoadState" => status.load_state = Some(value.to_string()),
            "MainPID" => status.main_pid = value.parse().ok(),
            _ => {}
        }
    }
    status
}

pub fn is_unit_active(unit: &str) -> Option<bool> {
    let output = run_command("systemctl", &["is-active", unit], Duration::from_secs(2)).ok()?;
    Some(output.stdout.trim() == "active")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_systemctl_show() {
        let s = parse_show(
            "ActiveState=active\nSubState=running\nUnitFileState=enabled\nLoadState=loaded\nMainPID=2755\n",
        );
        assert!(s.is_active());
        assert!(s.is_enabled());
        assert_eq!(s.main_pid, Some(2755));
    }

    #[test]
    fn masked_is_not_enabled() {
        let s = parse_show("UnitFileState=masked\nActiveState=inactive\n");
        assert!(!s.is_enabled());
        assert!(!s.is_active());
    }
}
