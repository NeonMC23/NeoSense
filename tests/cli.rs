use assert_cmd::Command;
use serde_json::Value;

#[test]
fn version_json_is_machine_readable() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["version", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["name"], "neosense");
    assert_eq!(value["schema"], "neosense.version/v1");
}

#[test]
fn status_json_is_valid_even_when_backend_is_missing() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["status", "--json"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.status/v1");
    assert!(value["warnings"].is_array());
    // The exit code may be 7 when NBFC is absent; stdout must remain valid JSON.
    assert!(output.status.code().is_some());
}

#[test]
fn sensors_json_is_a_snapshot() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["sensors", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.sensors/v1");
    assert!(value["sensors"].is_array());
    assert!(value["collected_at_ms"].is_number());
}

#[test]
fn sensor_show_json_has_a_schema_when_sensors_exist() {
    let list = Command::cargo_bin("neosense")
        .unwrap()
        .args(["sensors", "list", "--json"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&list.stdout).unwrap();
    let Some(id) = value["sensors"]
        .as_array()
        .and_then(|sensors| sensors.first())
        .and_then(|sensor| sensor["id"].as_str())
    else {
        return;
    };
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["sensors", "show", id, "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.sensor/v1");
    assert_eq!(value["id"], id);
}
#[test]
fn invalid_percentage_is_usage_error() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "fan",
            "set",
            "cpu",
            "150",
            "--yes",
            "--socket",
            "/nonexistent",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn manual_without_confirmation_is_safety_refusal() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["fan", "set", "cpu", "50", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(8));
}

#[test]
fn control_without_supervisor_is_backend_unavailable() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["fan", "auto", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn diagnose_json_is_valid() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["diagnose", "--json"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["checks"].is_array());
    assert!(value["environment"].is_object());
}

#[test]
fn permission_denied_is_reported_with_group_hint() {
    if unsafe { libc::geteuid() } == 0 {
        return; // root bypasses Unix socket permission checks
    }
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("restricted.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["fan", "auto", "--socket"])
        .arg(&socket)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("neosense"), "{stderr}");
    drop(listener);
}

#[test]
fn profile_catalogue_marks_curves_as_preview() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let profiles = value.as_array().unwrap();
    assert!(profiles.iter().any(|p| {
        p["profile"]["name"] == "safe-auto"
            && matches!(
                p["compatibility"].as_str(),
                Some("compatible" | "compatible_with_warnings")
            )
    }));
    assert!(
        profiles
            .iter()
            .any(|p| { p["profile"]["name"] == "quiet" && p["compatibility"] == "preview_only" })
    );
}

#[test]
fn preview_profile_cannot_be_applied() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "apply", "quiet", "--yes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9));
}

#[test]
fn fan_status_json_has_a_schema() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["fan", "status", "--json"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.fan_status/v1");
}

#[test]
fn service_status_json_has_a_schema() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["service", "status", "--json"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.service_status/v2");
}

#[test]
fn diagnose_json_has_a_schema() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["diagnose", "--json"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.diagnose/v1");
}

#[test]
fn profile_show_json_has_a_schema() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "show", "safe-auto", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.profile/v1");
    assert_eq!(value["profile"]["name"], "safe-auto");
}

#[test]
fn profile_preview_is_explicitly_non_writing() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "preview", "quiet", "--json"])
        .output()
        .unwrap();
    if output.status.code() == Some(7) {
        return; // NBFC is absent in this test environment
    }
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.profile_preview/v1");
    assert_eq!(value["mode"], "preview");
    assert_eq!(value["hardware_applied"], false);
    assert!(value["fan_targets"].is_array());
}

#[test]
fn profile_preview_rejects_profiles_without_curves() {
    for name in ["safe-auto", "custom"] {
        let output = Command::cargo_bin("neosense")
            .unwrap()
            .args(["profile", "preview", name])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(9), "{name}: {output:?}");
    }
}

#[test]
fn profile_select_requires_the_supervisor() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "select", "quiet", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn profile_state_requires_the_supervisor() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "state", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
}
#[test]
fn config_show_is_machine_readable() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["config", "show", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.config/v1");
    assert!(value["safety"]["min_percent"].as_u64().unwrap() >= 1);
}

#[test]
fn release_is_explicitly_unavailable() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["fan", "release", "--yes"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9));
}

#[test]
fn hold_without_supervisor_never_writes() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["fan", "hold", "--yes", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn tui_requires_a_terminal() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .arg("tui")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("interactive terminal"), "{stderr}");
}

#[test]
fn tui_rejects_json_mode() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["tui", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn tui_rejects_no_color_mode() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["tui", "--no-color"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
#[test]
fn diagnose_summary_is_structured_and_manual_warning_is_consolidated() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["diagnose", "--json"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let summary = value["summary"].as_object().unwrap();
    for key in [
        "overall",
        "operational",
        "supervisor",
        "fan_control_available",
        "thermal_reliable",
        "critical_issues",
        "warning_issues",
    ] {
        assert!(summary.contains_key(key), "missing summary.{key}");
    }
    let checks = value["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .filter(|check| check["id"].as_str().unwrap_or("").starts_with("manual."))
            .count()
            <= 1
    );
    assert!(checks.iter().all(|check| check["group"].is_string()));
}

#[test]
fn diagnostic_summary_and_consolidated_manual_check() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["diagnose", "--json"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let summary = value["summary"].as_object().unwrap();
    for key in [
        "overall",
        "operational",
        "supervisor",
        "fan_control_available",
        "thermal_reliable",
        "critical_issues",
        "warning_issues",
    ] {
        assert!(summary.contains_key(key), "missing summary.{key}");
    }
    let manual = value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["id"].as_str().unwrap_or("").starts_with("manual."))
        .count();
    assert!(manual <= 1, "NBFC version limitation repeated per fan");
    assert!(
        value["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|check| check["group"].is_string())
    );
}

#[test]
fn service_status_distinguishes_missing_socket() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "service",
            "status",
            "--socket",
            "/nonexistent-neosense.sock",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.is_empty());
}

#[test]
fn service_status_distinguishes_permission_denied() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("restricted.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["service", "status", "--socket"])
        .arg(&socket)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    drop(listener);
}

#[test]
fn shell_completions_are_generated() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["completions", "bash"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("_neosense"));
    assert!(!text.contains("\u{1b}"));
}

#[test]
fn sensors_watch_once_json_is_one_valid_snapshot() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["sensors", "watch", "--once", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.sensors/v1");
    assert!(value["sensors"].is_array());
    assert!(value["collected_at_ms"].is_number());
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.lines().count(), 1, "NDJSON must contain one object");
    assert!(!text.contains("Ctrl-C"));
}

#[test]
fn sensors_watch_rejects_zero_interval() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["sensors", "watch", "--interval", "0", "--once"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("at least 1 second"), "{stderr}");
}

#[test]
fn every_advertised_shell_completion_is_generated() {
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let output = Command::cargo_bin("neosense")
            .unwrap()
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(output.status.success(), "{shell} completion failed");
        assert!(!output.stdout.is_empty(), "{shell} completion is empty");
        assert!(
            !output.stdout.contains(&0x1b),
            "{shell} completion contains ANSI"
        );
    }
}
#[test]
fn completions_reject_json_mode() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["completions", "bash", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
