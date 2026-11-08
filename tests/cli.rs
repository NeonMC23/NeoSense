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
    assert!(value["sensors"].is_array());
    assert!(value["collected_at_ms"].is_number());
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
