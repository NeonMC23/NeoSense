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
    assert_eq!(value["control"]["schema"], "neosense.control_readiness/v1");
    assert!(value["control"]["stage"].is_string());
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
fn control_readiness_json_is_structured_and_does_not_claim_validation() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["control", "readiness", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.control_readiness/v1");
    assert!(value["stage"].is_string());
    assert!(value["mode"].is_string());
    assert!(value["requirements"].is_array());
    assert!(value["fans"].is_array());
    assert_eq!(value["physical_control_validated"], false);
    assert!(value["next_action"].is_string());
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
    assert!(value["control"]["fans"].is_array());
}

#[test]
fn diagnose_human_output_separates_manual_path_from_physical_control() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["diagnose"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Manual path"), "{text}");
    assert!(text.contains("RPM available"), "{text}");
    assert!(text.contains("RPM fresh"), "{text}");
    assert!(text.contains("RPM trusted"), "{text}");
    assert!(text.contains("Fan identity"), "{text}");
    assert!(text.contains("Physical control"), "{text}");
    assert!(!text.contains("Fan control       :"), "{text}");
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
fn profile_catalogue_reports_closed_loop_profiles_as_policy_gated() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "list", "--json", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let profiles = value.as_array().unwrap();
    let safe_auto = profiles
        .iter()
        .find(|p| p["profile"]["name"] == "safe-auto")
        .unwrap();
    assert!(matches!(
        safe_auto["compatibility"].as_str(),
        Some("compatible" | "compatible_with_warnings" | "partial" | "blocked_for_safety")
    ));
    let quiet = profiles
        .iter()
        .find(|p| p["profile"]["name"] == "quiet")
        .unwrap();
    assert_eq!(quiet["compatibility"], "blocked_for_safety");
    assert!(quiet["profile"]["curve"].is_object());
    assert!(quiet["profile"]["reason"].as_str().is_some());
}

#[test]
fn profile_apply_requires_supervisor_and_confirmation() {
    let no_confirm = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "apply", "quiet"])
        .output()
        .unwrap();
    assert_eq!(no_confirm.status.code(), Some(8));
    let no_supervisor = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "profile",
            "apply",
            "quiet",
            "--yes",
            "--socket",
            "/nonexistent",
        ])
        .output()
        .unwrap();
    assert_eq!(no_supervisor.status.code(), Some(7));
}

fn fan_status_response_with_ec() -> Value {
    let mut response = fan_status_response();
    let fans = response["result"]["nbfc"]["fans"].as_array_mut().unwrap();
    fans[0]["requested_speed"] = serde_json::json!(50.0);
    fans[0]["target_speed"] = serde_json::json!(55.0);
    fans[1]["requested_speed"] = serde_json::json!(0.0);
    fans[1]["target_speed"] = serde_json::json!(60.0);
    let sensors = response["result"]["sensors"]["sensors"]
        .as_array_mut()
        .unwrap();
    sensors.push(serde_json::json!({
        "id": "ec:fan0.rpm",
        "role": {"kind": "fan_rpm", "index": 0},
        "kind": "rpm",
        "value": 4225.0,
        "unit": "raw",
        "raw_value": "0x81:0x10",
        "raw_unit": "ec_u16_provisional",
        "provider": "ec",
        "chip": "acer_ec",
        "path": "/sys/kernel/debug/ec/ec0/io",
        "label": "EC tach 0 (provisional)",
        "device_id": "acer-ec:ec0",
        "sampled_at_ms": 1,
        "status": "ok",
        "health": {
            "detected": true,
            "readable": true,
            "valid": true,
            "fresh": false,
            "trust": "provisional",
            "usable_for_regulation": false,
            "usable_for_feedback": false,
            "physical_measurement": true,
            "usable_for_control": false
        },
        "error": null
    }));
    response
}

#[test]
fn fan_status_json_keeps_ec_tach_raw_and_clarifies_automatic_requested_state() {
    let (_dir, socket) = one_shot_supervisor(fan_status_response_with_ec());
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "fan",
            "status",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let channel = &value["rpm_feedback"][0];
    assert_eq!(channel["unit"], "raw");
    assert_eq!(channel["raw_unit"], "ec_u16_provisional");
    assert_eq!(channel["trust"], "provisional");
    assert_eq!(channel["usable_for_feedback"], false);
    assert_eq!(channel["verified_rpm"], false);
    assert_eq!(channel["value_semantics"], "raw_ec_word");
    assert_eq!(channel["value"], 4225.0);

    let cpu = &value["fan_semantics"][0];
    assert_eq!(cpu["auto_mode"], true);
    assert_eq!(cpu["requested_speed"], 50.0);
    assert_eq!(cpu["target_speed"], 55.0);
    assert_eq!(cpu["requested_speed_is_active"], false);
    assert_eq!(
        cpu["requested_speed_semantics"],
        "last_fixed_request_not_active"
    );
    assert_eq!(cpu["target_semantics"], "nbfc_automatic_curve_target");
    let gpu = &value["fan_semantics"][1];
    assert_eq!(gpu["requested_speed"], 0.0);
    assert_eq!(gpu["target_speed"], 60.0);
    assert_eq!(gpu["requested_speed_is_active"], false);
}

#[test]
fn fans_list_json_keeps_provisional_ec_channel_raw() {
    let (_dir, socket) = one_shot_supervisor(fan_status_response_with_ec());
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "fans",
            "list",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let channel = &value["rpm_channels"][0];
    assert_eq!(channel["unit"], "raw");
    assert_eq!(channel["raw_unit"], "ec_u16_provisional");
    assert_eq!(channel["trust"], "provisional");
    assert_eq!(channel["usable_for_feedback"], false);
    assert_eq!(channel["verified_rpm"], false);
}

#[test]
fn fan_status_human_output_labels_historical_requested_speed() {
    let (_dir, socket) = one_shot_supervisor(fan_status_response_with_ec());
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "fan",
            "status",
            "--no-color",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("Requested is the last fixed request"),
        "{text}"
    );
    assert!(text.contains("(last)"), "{text}");
    assert!(text.contains("(auto)"), "{text}");
    assert!(!text.contains("EC tach 0 (provisional) 4225 rpm"), "{text}");
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
    // Backend capability must never be reported as validated physical control.
    assert_eq!(value["summary"]["physical_control_validated"], false);
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
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "preview", "safe-auto"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9), "{output:?}");

    // There is no placeholder "custom" profile: users create named curves.
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["profile", "show", "custom", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
}

#[test]
fn custom_profile_cli_lifecycle_uses_an_isolated_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let run = |args: &[&str]| {
        Command::cargo_bin("neosense")
            .unwrap()
            .env("NEOSENSE_PROFILES_PATH", &path)
            .args(["--socket", "/nonexistent"])
            .args(args)
            .output()
            .unwrap()
    };

    let output = run(&[
        "profile",
        "custom",
        "create",
        "--name",
        "desk",
        "--title",
        "Desk curve",
        "--point",
        "40:25",
        "--point",
        "70:55",
        "--point",
        "90:100",
        "--fans",
        "cpu",
    ]);
    assert!(output.status.success(), "{output:?}");

    let output = run(&["profile", "custom", "list", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["schema"], "neosense.custom_profiles_status/v1");
    assert_eq!(status["profiles"][0]["name"], "desk");
    assert_eq!(status["profiles"][0]["fan_roles"][0], "cpu");

    let output = run(&["profile", "custom", "show", "desk", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(shown["schema"], "neosense.custom_profile/v1");
    assert_eq!(shown["profile"]["points"][1]["percent"], 55.0);

    let output = run(&["profile", "show", "desk", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(shown["profile"]["name"], "desk");
    assert_eq!(shown["profile"]["origin"], "custom");

    let output = run(&[
        "profile",
        "custom",
        "update",
        "desk",
        "--title",
        "Desk updated",
        "--point",
        "40:30",
        "--point",
        "70:60",
        "--point",
        "90:100",
    ]);
    assert!(output.status.success(), "{output:?}");
    let output = run(&["profile", "custom", "show", "desk", "--json"]);
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(shown["profile"]["title"], "Desk updated");
    assert_eq!(shown["profile"]["points"][0]["percent"], 30.0);

    let output = run(&["profile", "custom", "duplicate", "desk", "desk-copy"]);
    assert!(output.status.success(), "{output:?}");
    let output = run(&["profile", "custom", "rename", "desk-copy", "desk-renamed"]);
    assert!(output.status.success(), "{output:?}");
    let output = run(&["profile", "custom", "delete", "desk-renamed", "--yes"]);
    assert!(output.status.success(), "{output:?}");

    let output = run(&["profile", "custom", "delete", "desk"]);
    assert_eq!(output.status.code(), Some(8), "{output:?}");
    let output = run(&["profile", "custom", "delete", "desk", "--yes"]);
    assert!(output.status.success(), "{output:?}");
    let output = run(&["profile", "custom", "list", "--json"]);
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(status["profiles"].as_array().unwrap().is_empty());
}

#[test]
fn custom_profile_cli_rejects_invalid_curves_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .env("NEOSENSE_PROFILES_PATH", &path)
        .args([
            "--socket",
            "/nonexistent",
            "profile",
            "custom",
            "create",
            "--name",
            "Bad Name",
            "--point",
            "40:25",
            "--point",
            "80:80",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9), "{output:?}");
    assert!(!path.exists());
}

#[test]
fn custom_profile_validation_reads_a_file_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    let file = dir.path().join("candidate.json");
    std::fs::write(
        &file,
        serde_json::json!({
            "name": "candidate",
            "title": "Candidate",
            "description": "file-based",
            "points": [
                {"temperature_c": 40.0, "percent": 25.0},
                {"temperature_c": 80.0, "percent": 80.0}
            ],
            "min_percent": 20.0,
            "max_percent": 100.0,
            "hysteresis_c": 3.0,
            "max_step_percent": 10.0,
            "stale_after_ms": 3000,
            "fan_roles": ["cpu"]
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .env("NEOSENSE_PROFILES_PATH", &path)
        .args([
            "--socket",
            "/nonexistent",
            "profile",
            "custom",
            "validate",
            "--file",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!path.exists());
}

#[test]
fn corrupt_custom_store_is_reported_and_can_be_reset_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profiles.json");
    std::fs::write(&path, b"not json").unwrap();
    let run = |args: &[&str]| {
        Command::cargo_bin("neosense")
            .unwrap()
            .env("NEOSENSE_PROFILES_PATH", &path)
            .args(["--socket", "/nonexistent"])
            .args(args)
            .output()
            .unwrap()
    };

    let output = run(&["profile", "custom", "list", "--json"]);
    assert_eq!(output.status.code(), Some(9), "{output:?}");
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(status["error"].as_str().unwrap().contains("corrupt"));

    let output = run(&["profile", "custom", "reset"]);
    assert_eq!(output.status.code(), Some(8), "{output:?}");

    let output = run(&["profile", "custom", "reset", "--yes"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"version\": 1")
    );
    let preserved: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
        .collect();
    assert_eq!(preserved.len(), 1);
    assert_eq!(std::fs::read(preserved[0].path()).unwrap(), b"not json");

    let output = run(&["profile", "custom", "list", "--json"]);
    assert!(output.status.success(), "{output:?}");
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
fn cooling_presets_are_silent_balanced_and_maximum() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["preset", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.preset_list/v1");
    let presets = value["presets"].as_array().unwrap();
    assert_eq!(presets.len(), 3);
    for (name, expected) in [("silent", 25.0), ("balanced", 60.0), ("maximum", 100.0)] {
        let preset = presets.iter().find(|p| p["name"] == name).unwrap();
        assert_eq!(preset["cpu_percent"], expected);
        assert_eq!(preset["gpu_percent"], expected);
    }
    assert!(!presets.iter().any(|preset| preset["name"] == "turbo"));
    assert!(!presets.iter().any(|preset| preset["name"] == "performance"));
}

#[test]
fn preset_dry_run_does_not_claim_a_hardware_write() {
    let (_dir, socket) = one_shot_supervisor(serde_json::json!({
        "result": {
            "schema": "neosense.preset_result/v1",
            "preset": "balanced",
            "verified": false,
            "dry_run": true,
            "hardware_applied": false,
            "targets": [
                {"fan_index": 0, "fan_name": "CPU fan", "requested_percent": 60.0, "accepted_target_percent": null, "reported_percent": null},
                {"fan_index": 1, "fan_name": "GPU fan", "requested_percent": 60.0, "accepted_target_percent": null, "reported_percent": null}
            ],
            "message": "valid preset request; no write performed (dry-run)"
        }
    }));
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "preset",
            "apply",
            "balanced",
            "--dry-run",
            "--yes",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("DRY-RUN"), "{text}");
    assert!(text.contains("no write performed"), "{text}");
    assert!(!text.contains("targets confirmed"), "{text}");
}

#[test]
fn fixed_preset_requires_confirmation_before_ipc() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["preset", "apply", "silent", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(8));
}

#[test]
fn fixed_preset_without_supervisor_is_unavailable() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "preset",
            "apply",
            "silent",
            "--yes",
            "--socket",
            "/nonexistent",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
}
#[test]
fn hardware_identity_reports_per_fan_registers_and_invalid_readback() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let configs = dir.path().join("configs");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&configs).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        configs.join("Test.json"),
        serde_json::json!({
            "NotebookModel": "Test laptop",
            "Author": "Test",
            "CriticalTemperature": 95,
            "EcPollInterval": 1000,
            "ReadWriteWords": true,
            "FanConfigurations": [
                {
                    "ReadRegister": 19, "WriteRegister": 55,
                    "MinSpeedValue": 0, "MaxSpeedValue": 100,
                    "IndependentReadMinMaxValues": false,
                    "MinSpeedValueRead": 0, "MaxSpeedValueRead": 0,
                    "FanDisplayName": "CPU fan",
                    "TemperatureThresholds": [{"UpThreshold": 40, "DownThreshold": 0, "FanSpeed": 0}]
                },
                {
                    "ReadRegister": 21, "WriteRegister": 58,
                    "MinSpeedValue": 0, "MaxSpeedValue": 100,
                    "IndependentReadMinMaxValues": false,
                    "MinSpeedValueRead": 0, "MaxSpeedValueRead": 0,
                    "FanDisplayName": "GPU fan",
                    "TemperatureThresholds": [{"UpThreshold": 40, "DownThreshold": 0, "FanSpeed": 0}]
                }
            ],
            "RegisterWriteConfigurations": []
        })
        .to_string(),
    )
    .unwrap();
    let fake = bin.join("nbfc");
    std::fs::write(
        &fake,
        r#"#!/bin/sh
case "$1" in
  --version) echo "nbfc 0.5.4";;
  status) printf '%s\n' '{"ReadOnly":false,"SelectedConfigId":"Test","Fans":[{"Name":"CPU fan","AutoMode":true},{"Name":"GPU fan","AutoMode":true}]}';;
  sensors) printf '%s\n' '#0 (CPU fan)' 'Sensors: @CPU' 'Algorithm: Average' '#1 (GPU fan)' 'Sensors: @CPU' 'Algorithm: Average';;
  *) exit 1;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .env("PATH", path)
        .env("NEOSENSE_NBFC_CONFIG_DIRS", &configs)
        .args(["hardware", "identity", "--config", "Test", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.hardware_identity/v1");
    assert_eq!(value["selected_config"], "Test");
    assert_eq!(value["hardware_applied"], false);
    assert_eq!(value["fans"][0]["read_register"], 19);
    assert_eq!(value["fans"][0]["write_register"], 55);
    assert_eq!(value["fans"][1]["write_register"], 58);
    assert_eq!(value["fans"][0]["role"], "cpu");
    assert_eq!(value["fans"][1]["role"], "gpu");
    assert_eq!(value["fans"][0]["readback_configured"], false);
    assert!(matches!(
        value["feedback"]["rpm_source"].as_str(),
        Some("unavailable" | "hwmon" | "provisional_ec")
    ));
    assert!(value["feedback"]["rpm_channels"].is_array());
    for channel in value["feedback"]["rpm_channels"].as_array().unwrap() {
        let name = channel["name"].as_str().unwrap_or_default();
        if name.starts_with("Acer Fan ") {
            assert_eq!(channel["unit"], "rpm");
            assert_eq!(channel["trust"], "verified");
        } else {
            // The current kernel may expose provisional EC tach words instead
            // of the historical Acer hwmon channels.
            assert!(name.starts_with("EC tach "), "{name}");
            assert_eq!(channel["unit"], "raw");
            assert_eq!(channel["raw_unit"], "ec_u16_provisional");
            assert_eq!(channel["trust"], "provisional");
        }
        assert_eq!(channel["physical_measurement"], true);
        assert_eq!(channel["usable_for_regulation"], false);
    }
    assert_eq!(value["feedback"]["ec_readback"], "uncalibrated_write_range");
    assert_eq!(value["feedback"]["physical_identity"], "logical_only");
    assert!(
        value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap_or("").contains("CurrentSpeed"))
    );
}

#[test]
fn hardware_identity_uses_supervisor_state_not_a_direct_nbfc_connection() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let configs = dir.path().join("configs");
    let empty_bin = dir.path().join("empty-bin");
    std::fs::create_dir_all(&configs).unwrap();
    std::fs::create_dir_all(&empty_bin).unwrap();
    std::fs::write(
        configs.join("Test.json"),
        serde_json::json!({
            "NotebookModel": "Test laptop",
            "Author": "Test",
            "CriticalTemperature": 95,
            "EcPollInterval": 1000,
            "ReadWriteWords": true,
            "FanConfigurations": [
                {
                    "ReadRegister": 19, "WriteRegister": 55,
                    "MinSpeedValue": 0, "MaxSpeedValue": 100,
                    "IndependentReadMinMaxValues": false,
                    "MinSpeedValueRead": 0, "MaxSpeedValueRead": 0,
                    "FanDisplayName": "CPU fan",
                    "TemperatureThresholds": [{"UpThreshold": 40, "DownThreshold": 0, "FanSpeed": 0}]
                },
                {
                    "ReadRegister": 21, "WriteRegister": 58,
                    "MinSpeedValue": 0, "MaxSpeedValue": 100,
                    "IndependentReadMinMaxValues": false,
                    "MinSpeedValueRead": 0, "MaxSpeedValueRead": 0,
                    "FanDisplayName": "GPU fan",
                    "TemperatureThresholds": [{"UpThreshold": 40, "DownThreshold": 0, "FanSpeed": 0}]
                }
            ],
            "RegisterWriteConfigurations": []
        })
        .to_string(),
    )
    .unwrap();

    let sensors = serde_json::to_value(neosense::model::SensorSnapshot {
        sensors: vec![
            neosense::model::SensorSample::new(
                "acer_ec:fan0.rpm".into(),
                neosense::model::SensorRole::FanRpm(0),
                neosense::model::SensorKind::Rpm,
                Some(4225.0),
                neosense::model::Unit::Raw,
                Some("0x81:0x10".into()),
                "ec_u16_provisional",
                neosense::model::SensorProvider::Ec,
                "acer_ec".into(),
                Some("/sys/kernel/debug/ec/ec0/io".into()),
                Some("EC tach 0 (provisional)".into()),
                Some("acer-ec:ec0".into()),
            ),
            neosense::model::SensorSample::new(
                "acer_ec:fan1.rpm".into(),
                neosense::model::SensorRole::FanRpm(1),
                neosense::model::SensorKind::Rpm,
                Some(5000.0),
                neosense::model::Unit::Raw,
                Some("0x88:0x13".into()),
                "ec_u16_provisional",
                neosense::model::SensorProvider::Ec,
                "acer_ec".into(),
                Some("/sys/kernel/debug/ec/ec0/io".into()),
                Some("EC tach 1 (provisional)".into()),
                Some("acer-ec:ec0".into()),
            ),
        ],
        errors: Vec::new(),
        collected_at_ms: neosense::model::now_ms(),
    })
    .unwrap();

    let socket = dir.path().join("supervisor.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut line = String::new();
            {
                let mut reader = BufReader::new(&stream);
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
            }
            let id = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|value| value.get("id").and_then(Value::as_u64))
                .unwrap_or(0);
            let response = serde_json::json!({
                "id": id,
                "ok": true,
                "result": {
                    "nbfc_version": "0.5.4",
                    "nbfc": {
                        "version": null,
                        "read_only": false,
                        "selected_config": "Test",
                        "fans": [
                            {"index": 0, "name": "CPU fan", "temperature": 55.0, "critical_temperature": 95.0, "auto_mode": true, "critical": false, "current_speed": 100.0, "target_speed": 45.0, "requested_speed": 100.0, "speed_steps": 100},
                            {"index": 1, "name": "GPU fan", "temperature": 55.0, "critical_temperature": 95.0, "auto_mode": true, "critical": false, "current_speed": 100.0, "target_speed": 50.0, "requested_speed": 0.0, "speed_steps": 100}
                        ],
                        "warnings": []
                    },
                    "nbfc_runtime": {
                        "schema": "neosense.nbfc_runtime/v1",
                        "pid": 1234,
                        "command_line": ["nbfc_service", "-f"],
                        "config_path": "/etc/nbfc/nbfc.json",
                        "configured_backend": null,
                        "observed_backend": "ec_sys",
                        "effective_backend": "ec_sys",
                        "confidence": "verified",
                        "endpoints": [
                            {"fd": 5, "path": "/sys/kernel/debug/ec/ec0/io", "backend": "ec_sys"}
                        ],
                        "evidence": ["fd 5 open"],
                        "probe_errors": []
                    },
                    "service": {
                        "active_state": "active",
                        "sub_state": "running",
                        "unit_file_state": "enabled",
                        "load_state": "loaded",
                        "main_pid": 1234
                    },
                    "sensor_config": {
                        "fans": [
                            {"index": 0, "name": "CPU fan", "sensors": ["@CPU"], "algorithm": "Average"},
                            {"index": 1, "name": "GPU fan", "sensors": ["@CPU"], "algorithm": "Average"}
                        ]
                    },
                    "sensors": sensors,
                    "last_error": null
                }
            });
            let _ = writeln!(stream, "{response}");
        }
    });

    // No `nbfc` executable is on PATH: any direct backend call would fail.
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .env("PATH", &empty_bin)
        .env("NEOSENSE_NBFC_CONFIG_DIRS", &configs)
        .args([
            "hardware",
            "identity",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_config"], "Test");
    assert_eq!(value["fans"].as_array().unwrap().len(), 2);
    assert_eq!(value["fans"][1]["write_register"], 58);
    assert_eq!(value["runtime"]["effective_backend"], "ec_sys");
    assert_eq!(value["runtime"]["confidence"], "verified");
    assert_eq!(value["fans"][0]["readback_evidence"]["ec_channel"], 0);
    assert_eq!(
        value["fans"][0]["readback_evidence"]["outside_effective_range"],
        true
    );
    assert_eq!(
        value["fans"][0]["readback_evidence"]["interpretation"],
        "write_range_clamped"
    );
    assert_eq!(
        value["fans"][0]["readback_evidence"]["backend_current_speed_percent"],
        100.0
    );

    let human = Command::cargo_bin("neosense")
        .unwrap()
        .env("PATH", &empty_bin)
        .env("NEOSENSE_NBFC_CONFIG_DIRS", &configs)
        .args(["hardware", "identity", "--socket", socket.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(human.status.success(), "{human:?}");
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("raw EC 4225"), "{text}");
    assert!(text.contains("4225 raw"), "{text}");
    assert!(text.contains("clamped"), "{text}");
    assert!(!text.contains("4225 rpm"), "{text}");
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
    assert_eq!(value["control"]["schema"], "neosense.control_readiness/v1");
    assert_eq!(value["control"]["physical_control_validated"], false);
    let summary = value["summary"].as_object().unwrap();
    for key in [
        "overall",
        "operational",
        "supervisor",
        "fan_control_available",
        "physical_feedback_available",
        "physical_feedback_fresh",
        "physical_feedback_trusted",
        "physical_identity_verified",
        "physical_control_validated",
        "control_stage",
        "control_mode",
        "all_fans_eligible",
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
    assert_eq!(value["control"]["schema"], "neosense.control_readiness/v1");
    assert_eq!(value["control"]["physical_control_validated"], false);
    let summary = value["summary"].as_object().unwrap();
    for key in [
        "overall",
        "operational",
        "supervisor",
        "fan_control_available",
        "physical_feedback_available",
        "physical_feedback_fresh",
        "physical_feedback_trusted",
        "physical_identity_verified",
        "physical_control_validated",
        "control_stage",
        "control_mode",
        "all_fans_eligible",
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

/// Serve one supervisor request with a fixed result document.
fn one_shot_supervisor(mut response: Value) -> (tempfile::TempDir, std::path::PathBuf) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("supervisor.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut line = String::new();
        {
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
        }
        let id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_u64))
            .unwrap_or(0);
        response["id"] = Value::from(id);
        response["ok"] = Value::from(response.get("error").is_none());
        let _ = writeln!(stream, "{response}");
    });
    (dir, socket)
}

#[test]
fn sensors_list_comes_from_supervisor_ipc_not_local_hardware() {
    let dir = tempfile::tempdir().unwrap();
    let empty_bin = dir.path().join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();
    let (_supervisor_dir, socket) = one_shot_supervisor(serde_json::json!({
        "result": {
            "nbfc_version": "0.5.4",
            "nbfc": null,
            "service": {
                "active_state": "active",
                "sub_state": "running",
                "unit_file_state": "enabled",
                "load_state": "loaded",
                "main_pid": 1
            },
            "sensors": {
                "sensors": [{
                    "id": "sentinel:one",
                    "role": {"kind": "cpu_package"},
                    "kind": "temperature",
                    "value": 42.0,
                    "unit": "celsius",
                    "raw_value": "42000",
                    "raw_unit": "millidegree_c",
                    "provider": "hwmon",
                    "chip": "sentinel-chip",
                    "path": "/sys/sentinel/temp1_input",
                    "label": "Package id 0",
                    "device_id": "/sys/sentinel",
                    "sampled_at_ms": 1234,
                    "status": "ok",
                    "health": {
                        "detected": true,
                        "readable": true,
                        "valid": true,
                        "fresh": true,
                        "trust": "verified",
                        "usable_for_regulation": true,
                        "usable_for_feedback": false,
                        "physical_measurement": true,
                        "usable_for_control": true
                    },
                    "error": null
                }],
                "errors": [],
                "collected_at_ms": 1234
            },
            "last_error": null
        }
    }));
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .env("PATH", &empty_bin)
        .args([
            "sensors",
            "list",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["source"], "supervisor");
    assert_eq!(value["sensors"][0]["id"], "sentinel:one");
    assert_eq!(value["sensors"][0]["chip"], "sentinel-chip");
    assert_eq!(value["collected_at_ms"], 1234);
}

#[test]
fn sensors_list_without_supervisor_reports_unavailable_not_local_data() {
    let dir = tempfile::tempdir().unwrap();
    let empty_bin = dir.path().join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();
    let socket = dir.path().join("missing.sock");
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .env("PATH", &empty_bin)
        .args([
            "sensors",
            "list",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["source"], "unavailable");
    assert!(value["sensors"].as_array().unwrap().is_empty());
    assert!(
        value["source_error"]
            .as_str()
            .unwrap_or_default()
            .contains("does not access hardware directly")
    );
}

#[test]
fn hardware_list_reads_configuration_files_without_invoking_nbfc() {
    let dir = tempfile::tempdir().unwrap();
    let configs = dir.path().join("configs");
    let empty_bin = dir.path().join("empty-bin");
    std::fs::create_dir_all(&configs).unwrap();
    std::fs::create_dir_all(&empty_bin).unwrap();
    std::fs::write(configs.join("Alpha.json"), "{}").unwrap();
    std::fs::write(configs.join("Beta.json"), "{}").unwrap();
    std::fs::write(configs.join("ignored.txt"), "x").unwrap();
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .env("PATH", &empty_bin)
        .env("NEOSENSE_NBFC_CONFIG_DIRS", &configs)
        .args(["hardware", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["source"], "configuration_directories");
    assert_eq!(value["configs"], serde_json::json!(["Alpha", "Beta"]));
}

#[test]
fn hardware_validate_reports_missing_acpiexec_as_a_typed_limitation() {
    let (_supervisor_dir, socket) = one_shot_supervisor(serde_json::json!({
        "error": {
            "code": "analysis_unavailable",
            "message": "read-only configuration analysis is unavailable: acpiexec is missing; no fan write was performed",
            "details": ["acpica-tools is optional"]
        }
    }));
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "hardware",
            "validate",
            "Test",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], 7);
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("acpiexec")
    );
    assert!(!value["error"]["details"].as_array().unwrap().is_empty());
}

#[test]
fn hardware_validate_uses_supervisor_ipc_not_the_nbfc_cli() {
    let dir = tempfile::tempdir().unwrap();
    let empty_bin = dir.path().join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();
    let (_supervisor_dir, socket) = one_shot_supervisor(serde_json::json!({
        "result": {
            "schema": "neosense.hardware_validation/v1",
            "name": "Test",
            "analysis": {"ok": true, "writes": 0},
            "hardware_applied": false
        }
    }));
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .env("PATH", &empty_bin)
        .args([
            "hardware",
            "validate",
            "Test",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.hardware_validation/v1");
    assert_eq!(value["analysis"]["ok"], true);
    assert_eq!(value["hardware_applied"], false);
}

#[test]
fn calibration_readiness_is_read_only_and_fail_closed_without_a_supervisor() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "calibration",
            "readiness",
            "--json",
            "--socket",
            "/nonexistent-neosense-calibration.sock",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.calibration_readiness/v1");
    assert_eq!(value["state"], "blocked");
    assert_eq!(value["informational_only"], true);
    assert_eq!(value["test_authorized"], false);
    assert_eq!(value["protocol"]["experimental_min_percent"], 50);
    assert_eq!(value["protocol"]["cpu_max_temp_c"], 80.0);
    assert_eq!(value["protocol"]["gpu_max_temp_c"], 75.0);
    assert_eq!(value["protocol"]["test_authorized"], false);
    assert_eq!(value["requirements"][0]["id"], "runtime_backend");
    assert_eq!(value["requirements"][0]["status"], "missing");
    assert!(
        value["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker.as_str().unwrap_or("").contains("Runtime backend"))
    );
}

#[test]
fn calibration_readiness_consumes_supervisor_runtime_evidence_read_only() {
    let (_supervisor_dir, socket) = one_shot_supervisor(serde_json::json!({
        "result": {
            "nbfc_version": "0.5.4",
            "capabilities": {
                "version": {"major": 0, "minor": 5, "patch": 4},
                "json_status": true,
                "reset_ec": true,
                "config_apply": false,
                "protected_manual": true,
                "access_controlled": true
            },
            "nbfc": {
                "version": null,
                "read_only": false,
                "selected_config": "Test",
                "fans": [
                    {"index": 0, "name": "CPU fan", "temperature": 55.0, "critical_temperature": 95.0, "auto_mode": true, "critical": false, "current_speed": 100.0, "target_speed": 35.0, "requested_speed": 100.0, "speed_steps": 100},
                    {"index": 1, "name": "GPU fan", "temperature": 55.0, "critical_temperature": 95.0, "auto_mode": true, "critical": false, "current_speed": 100.0, "target_speed": 40.0, "requested_speed": 0.0, "speed_steps": 100}
                ],
                "warnings": []
            },
            "nbfc_runtime": {
                "schema": "neosense.nbfc_runtime/v1",
                "pid": 2393,
                "command_line": ["nbfc_service", "-f"],
                "config_path": "/etc/nbfc/nbfc.json",
                "configured_backend": null,
                "observed_backend": "ec_sys",
                "effective_backend": "ec_sys",
                "confidence": "verified",
                "endpoints": [{"fd": 18, "path": "/sys/kernel/debug/ec/ec0/io", "backend": "ec_sys"}],
                "evidence": ["fd 18 open"],
                "probe_errors": []
            },
            "last_validation": null,
            "service": {
                "active_state": "active",
                "sub_state": "running",
                "unit_file_state": "enabled",
                "load_state": "loaded",
                "main_pid": 1
            },
            "sensor_config": {"fans": [
                {"index": 0, "name": "CPU fan", "sensors": ["@CPU"], "algorithm": "Average"},
                {"index": 1, "name": "GPU fan", "sensors": ["@CPU"], "algorithm": "Average"}
            ]},
            "last_error": null
        }
    }));
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "calibration",
            "readiness",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["state"], "blocked");
    assert_eq!(value["test_authorized"], false);
    let runtime = value["requirements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|requirement| requirement["id"] == "runtime_backend")
        .unwrap();
    assert_eq!(runtime["status"], "satisfied");
    assert!(runtime["detail"].as_str().unwrap().contains("fd 18"));
    let rpm = value["requirements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|requirement| requirement["id"] == "independent_rpm")
        .unwrap();
    assert_eq!(rpm["status"], "missing");
}

#[test]
fn calibration_analysis_accepts_perfect_paired_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calibration.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "schema": "neosense.calibration_input/v1",
            "instrument": "test tachometer",
            "instrument_uncertainty": 1.0,
            "reference_kind": "rpm",
            "samples": [
                {"reference": 3000.0, "acer_fan1": 3000.0, "acer_fan2": 3000.0},
                {"reference": 4000.0, "acer_fan1": 4000.0, "acer_fan2": 4000.0},
                {"reference": 5000.0, "acer_fan1": 5000.0, "acer_fan2": 5000.0},
                {"reference": 6000.0, "acer_fan1": 6000.0, "acer_fan2": 6000.0},
                {"reference": 7000.0, "acer_fan1": 7000.0, "acer_fan2": 7000.0}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "calibration",
            "analyze",
            "--input",
            path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.calibration_analysis/v1");
    assert_eq!(value["absolute_rpm_calibration"], true);
    assert_eq!(value["channels"].as_array().unwrap().len(), 2);
    assert!(value["channels"][0]["accepted"].as_bool().unwrap());
}

#[test]
fn calibration_analysis_reports_unmet_acceptance_instead_of_claiming_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calibration.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "reference_kind": "rpm",
            "samples": [
                {"reference": 3100.0, "acer_fan1": 3000.0, "acer_fan2": 4000.0},
                {"reference": 4100.0, "acer_fan1": 4000.0, "acer_fan2": 5000.0},
                {"reference": 5100.0, "acer_fan1": 5000.0, "acer_fan2": 6000.0},
                {"reference": 6100.0, "acer_fan1": 6000.0, "acer_fan2": 7000.0},
                {"reference": 7100.0, "acer_fan1": 7000.0, "acer_fan2": 8000.0}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "calibration",
            "analyze",
            "--input",
            path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["absolute_rpm_calibration"], false);
    assert!(
        value["channels"][0]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason.as_str().unwrap_or("").contains("intercept"))
    );
}

#[test]
fn legacy_set_rejects_invalid_percent_before_any_backend_access() {
    for args in [
        vec!["set", "cpu", "150", "--yes"],
        vec!["set", "both", "-1", "--yes"],
    ] {
        let output = Command::cargo_bin("neosense")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn legacy_manual_and_presets_require_confirmation() {
    for args in [
        vec!["manual"],
        vec!["maximum"],
        vec!["turbo"],
        vec!["performance"],
        vec!["balanced"],
        vec!["silent"],
    ] {
        let output = Command::cargo_bin("neosense")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(8), "{output:?}");
    }
}

#[test]
fn legacy_auto_without_supervisor_is_backend_unavailable() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["auto", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn legacy_commands_are_discoverable_in_help() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    for command in [
        "auto",
        "manual",
        "set",
        "maximum",
        "turbo",
        "performance",
        "balanced",
        "silent",
    ] {
        assert!(text.contains(command), "missing {command}: {text}");
    }
}

fn fan_status_response() -> Value {
    serde_json::json!({
        "result": {
            "nbfc_version": "0.5.4",
            "capabilities": {
                "version": {"major": 0, "minor": 5, "patch": 4},
                "json_status": true,
                "reset_ec": true,
                "config_apply": false,
                "protected_manual": true,
                "access_controlled": true,
                "socket": null
            },
            "nbfc": {
                "version": null,
                "read_only": false,
                "selected_config": "Test",
                "fans": [
                    {"index": 0, "name": "CPU fan", "temperature": 60.0, "critical_temperature": 95.0,
                     "auto_mode": true, "critical": false, "current_speed": 40.0,
                     "target_speed": 40.0, "requested_speed": -1.0, "speed_steps": 100},
                    {"index": 1, "name": "GPU fan", "temperature": 60.0, "critical_temperature": 95.0,
                     "auto_mode": true, "critical": false, "current_speed": 50.0,
                     "target_speed": 50.0, "requested_speed": -1.0, "speed_steps": 100}
                ],
                "warnings": []
            },
            "service": {"active_state": "active", "sub_state": "running",
                        "unit_file_state": "enabled", "load_state": "loaded", "main_pid": 1},
            "sensor_config": {"fans": [
                {"index": 0, "name": "CPU fan", "sensors": ["@CPU"], "algorithm": "Average"},
                {"index": 1, "name": "GPU fan", "sensors": ["@CPU"], "algorithm": "Average"}
            ]},
            "sensors": {
                "sensors": [
                    {"id": "test:cpu", "role": {"kind": "cpu_package"}, "kind": "temperature",
                     "value": 60.0, "unit": "celsius", "raw_value": "60000", "raw_unit": "millidegree_c",
                     "provider": "hwmon", "chip": "coretemp", "path": "/sys/test/cpu",
                     "label": "Package id 0", "device_id": null, "sampled_at_ms": 1,
                     "status": "ok", "health": {"detected": true, "readable": true, "valid": true,
                     "fresh": true, "trust": "verified", "usable_for_regulation": true,
                     "usable_for_feedback": false, "physical_measurement": true,
                     "usable_for_control": true}, "error": null},
                    {"id": "test:gpu", "role": {"kind": "gpu_die", "index": 0}, "kind": "temperature",
                     "value": 55.0, "unit": "celsius", "raw_value": "55000", "raw_unit": "millidegree_c",
                     "provider": "nvidia", "chip": "test", "path": null,
                     "label": "temperature.gpu", "device_id": "test", "sampled_at_ms": 1,
                     "status": "ok", "health": {"detected": true, "readable": true, "valid": true,
                     "fresh": true, "trust": "verified", "usable_for_regulation": true,
                     "usable_for_feedback": false, "physical_measurement": true,
                     "usable_for_control": true}, "error": null}
                ],
                "errors": [],
                "collected_at_ms": 1
            },
            "last_error": null
        }
    })
}

#[test]
fn fans_list_json_is_typed_and_keeps_identity_unknown() {
    let (_dir, socket) = one_shot_supervisor(fan_status_response());
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "fans",
            "list",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.fan_inventory/v1");
    assert_eq!(value["backend"]["name"], "nbfc");
    assert_eq!(value["fans"].as_array().unwrap().len(), 2);
    assert_eq!(value["fans"][0]["physical_identity"], "unknown");
    assert_eq!(value["fans"][0]["role"], "cpu");
    assert_eq!(value["fans"][1]["role"], "gpu");
    assert_eq!(value["fans"][0]["rpm_mapping"], "unmapped");
    assert!(value["rpm_channels"].is_array());
    assert!(
        value["fans"][1]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason.as_str().unwrap_or("").contains("@CPU"))
    );
}

#[test]
fn fans_show_json_validates_index_and_schema() {
    let (_dir, socket) = one_shot_supervisor(fan_status_response());
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args([
            "fans",
            "show",
            "0",
            "--json",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "neosense.fan/v1");
    assert_eq!(value["fan"]["index"], 0);
    assert_eq!(value["fan"]["physical_identity"], "unknown");
}

#[test]
fn fans_and_index_control_parse_and_fail_conservatively_without_supervisor() {
    for args in [vec!["fans", "list"], vec!["fans", "show", "0"]] {
        let output = Command::cargo_bin("neosense")
            .unwrap()
            .args(args)
            .args(["--socket", "/nonexistent"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(7), "{output:?}");
    }
    for args in [
        vec!["set", "fan", "abc", "50"],
        vec!["set", "fan", "0", "150"],
    ] {
        let output = Command::cargo_bin("neosense")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
    }
    let no_confirm = Command::cargo_bin("neosense")
        .unwrap()
        .args(["set", "fan", "0", "50"])
        .output()
        .unwrap();
    assert_eq!(no_confirm.status.code(), Some(8));
    let auto_bad = Command::cargo_bin("neosense")
        .unwrap()
        .args(["auto", "fan", "abc"])
        .output()
        .unwrap();
    assert_eq!(auto_bad.status.code(), Some(2));
    let auto_missing = Command::cargo_bin("neosense")
        .unwrap()
        .args(["auto", "fan", "0", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(auto_missing.status.code(), Some(7));
}

#[test]
fn interactive_validation_refuses_non_tty_without_hardware_access() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["fan", "validate", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("interactive terminal"), "{text}");
}

#[test]
fn interactive_validation_refuses_json_mode() {
    let output = Command::cargo_bin("neosense")
        .unwrap()
        .args(["fan", "validate", "--json", "--socket", "/nonexistent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("not compatible with --json"), "{text}");
}

#[cfg(unix)]
#[test]
fn pty_cpu_validation_reaches_observation_and_exits_success() {
    use neosense::ipc::{Request, Response, read_message, write_message};
    use neosense::validation::{
        ExpectedSide, RecoveryEvidence, ValidationFan, ValidationLimits, ValidationPreflight,
        ValidationSession, ValidationState, ValidationStep, ValidationStepSpec,
        ValidationTelemetry,
    };
    use std::io::BufReader;
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::process::{Command as ProcessCommand, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("neosensed.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let listener = Arc::new(listener);
    let stop = Arc::new(AtomicBool::new(false));
    let fan = ValidationFan {
        index: 0,
        backend_id: "nbfc:0".into(),
        name: "CPU fan".into(),
        role: neosense::policy::FanRole::Cpu,
    };
    let limits = ValidationLimits {
        step_duration_ms: 20,
        step_timeout_ms: 1000,
        session_ttl_ms: 10_000,
        ..ValidationLimits::default()
    };
    let mut session = ValidationSession::new(
        "pty-session".into(),
        fan.clone(),
        ExpectedSide::Left,
        limits.clone(),
        1,
        Some("test".into()),
        Some("0.5.4".into()),
    );
    session.state = ValidationState::AwaitingObservation;
    session.recovery = Some(RecoveryEvidence {
        attempted: true,
        backend_auto: true,
        ec_verified: Some(false),
        detail: "backend automatic; EC unverified".into(),
        completed_at_ms: 2,
    });
    let preflight = ValidationPreflight {
        schema: neosense::validation::PREFLIGHT_SCHEMA.into(),
        ready: true,
        blockers: Vec::new(),
        warnings: Vec::new(),
        fan: fan.clone(),
        expected_side: ExpectedSide::Left,
        other_fan: None,
        other_fan_auto: true,
        steps: ValidationStep::SEQUENCE
            .into_iter()
            .map(|step| ValidationStepSpec {
                step,
                requested_percent: step.percent(),
                duration_ms: limits.step_duration_ms,
            })
            .collect(),
        limits: limits.clone(),
        telemetry: ValidationTelemetry {
            collected_at_ms: 1,
            cpu: None,
            gpu: None,
            nbfc_fans: Vec::new(),
            ec_values: Vec::new(),
        },
        safety_state: Some("safe_auto".into()),
        selected_config: Some("test".into()),
        nbfc_version: Some("0.5.4".into()),
    };
    let server_listener = Arc::clone(&listener);
    let server_stop = Arc::clone(&stop);
    let server_session = session.clone();
    let server = std::thread::spawn(move || {
        server_listener.set_nonblocking(true).unwrap();
        while !server_stop.load(Ordering::Relaxed) {
            match server_listener.accept() {
                Ok((mut stream, _)) => {
                    let mut reader = BufReader::new(&stream);
                    let request: Request = match read_message(&mut reader) {
                        Ok(request) => request,
                        Err(_) => continue,
                    };
                    let response = match request.method.as_str() {
                        "status.get" => Response::ok(
                            request.id,
                            serde_json::json!({
                                "health": {"healthy": true},
                                "nbfc": {
                                    "version": null,
                                    "read_only": false,
                                    "selected_config": "test",
                                    "fans": [
                                        {"index":0,"name":"CPU fan","temperature":55.0,"critical_temperature":95.0,"auto_mode":true,"critical":false,"current_speed":40.0,"target_speed":40.0,"requested_speed":-1.0,"speed_steps":100},
                                        {"index":1,"name":"GPU fan","temperature":55.0,"critical_temperature":95.0,"auto_mode":true,"critical":false,"current_speed":40.0,"target_speed":40.0,"requested_speed":-1.0,"speed_steps":100}
                                    ],
                                    "warnings": []
                                },
                                "sensors": {"sensors":[],"errors":[],"collected_at_ms":1},
                                "sensor_config": null,
                                "service": {},
                                "last_error": null
                            }),
                        ),
                        "validation.preflight" => Response::ok(request.id, preflight.clone()),
                        "validation.begin" => Response::ok(request.id, server_session.clone()),
                        "validation.status" => Response::ok(
                            request.id,
                            serde_json::json!({"session": server_session, "last": null, "limits": limits}),
                        ),
                        "validation.observe" => {
                            let mut observed = server_session.clone();
                            observed
                                .record_observation(
                                    neosense::validation::ObservationAnswer::Yes,
                                    None,
                                    3,
                                )
                                .unwrap();
                            Response::ok(request.id, observed)
                        }
                        "validation.cancel" | "validation.abort" => {
                            Response::ok(request.id, serde_json::json!({"aborted": false}))
                        }
                        _ => Response::error(
                            request.id,
                            neosense::ipc::ApiError::new("unknown_method", "unexpected method"),
                        ),
                    };
                    let _ = write_message(&mut stream, &response);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(rc, 0);
    let master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let mut command = ProcessCommand::new(assert_cmd::cargo::cargo_bin("neosense"));
    command
        .args([
            "fan",
            "validate",
            "cpu",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()));
    let mut child = command.spawn().unwrap();
    {
        use std::io::Write;
        let mut master = master;
        master.write_all(b"yes\nyes\n").unwrap();
        master.flush().unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "CLI did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    stop.store(true, Ordering::Relaxed);
    let _ = server.join();
    assert_eq!(status.code(), Some(0));
}

#[cfg(unix)]
#[test]
fn pty_validation_authorization_decline_exits_without_begin() {
    use neosense::ipc::{ApiError, Request, Response, read_message, write_message};
    use std::io::BufReader;
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::process::{Command as ProcessCommand, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("neosensed.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let listener = Arc::new(listener);
    let stop = Arc::new(AtomicBool::new(false));
    let begin_seen = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server_begin = Arc::clone(&begin_seen);
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        while !server_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut reader = BufReader::new(&stream);
                    let request: Request = match read_message(&mut reader) {
                        Ok(request) => request,
                        Err(_) => continue,
                    };
                    let response = match request.method.as_str() {
                        "status.get" => Response::ok(
                            request.id,
                            serde_json::json!({
                                "health": {"healthy": true},
                                "nbfc": {
                                    "version": null, "read_only": false, "selected_config": "test",
                                    "fans": [
                                        {"index":0,"name":"CPU fan","temperature":55.0,"critical_temperature":95.0,"auto_mode":true,"critical":false,"current_speed":40.0,"target_speed":40.0,"requested_speed":-1.0,"speed_steps":100},
                                        {"index":1,"name":"GPU fan","temperature":55.0,"critical_temperature":95.0,"auto_mode":true,"critical":false,"current_speed":40.0,"target_speed":40.0,"requested_speed":-1.0,"speed_steps":100}
                                    ], "warnings": []
                                },
                                "sensors": {"sensors":[],"errors":[],"collected_at_ms":1},
                                "sensor_config": null, "service": {}, "last_error": null
                            }),
                        ),
                        "validation.preflight" => Response::ok(
                            request.id,
                            serde_json::json!({
                                "schema": "neosense.validation_preflight/v1",
                                "ready": true, "blockers": [], "warnings": [],
                                "fan": {"index": 0, "backend_id": "nbfc:0", "name": "CPU fan", "role": "cpu"},
                                "expected_side": "left", "other_fan": null, "other_fan_auto": true,
                                "steps": [
                                    {"step": "fifty", "requested_percent": 50.0, "duration_ms": 20},
                                    {"step": "seventy_five", "requested_percent": 75.0, "duration_ms": 20},
                                    {"step": "hundred", "requested_percent": 100.0, "duration_ms": 20}
                                ],
                                "limits": {"step_duration_ms":20,"step_timeout_ms":1000,"session_ttl_ms":10000,"cpu_max_temp_c":80.0,"gpu_max_temp_c":75.0,"allow_zero_percent":false},
                                "telemetry": {"collected_at_ms":1,"cpu":null,"gpu":null,"nbfc_fans":[],"ec_values":[]},
                                "safety_state": "safe_auto", "selected_config": "test", "nbfc_version": "0.5.4"
                            }),
                        ),
                        "validation.begin" => {
                            server_begin.store(true, Ordering::SeqCst);
                            Response::error(
                                request.id,
                                ApiError::new("unexpected", "begin must not be called"),
                            )
                        }
                        "validation.cancel" => Response::ok(
                            request.id,
                            serde_json::json!({"cancelled": true, "hardware_command_sent": false}),
                        ),
                        _ => Response::error(
                            request.id,
                            ApiError::new("unknown_method", "unexpected method"),
                        ),
                    };
                    let _ = write_message(&mut stream, &response);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let mut command = ProcessCommand::new(assert_cmd::cargo::cargo_bin("neosense"));
    command
        .args([
            "fan",
            "validate",
            "cpu",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()));
    let mut child = command.spawn().unwrap();
    {
        use std::io::Write;
        master.write_all(b"no\n").unwrap();
        master.flush().unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "CLI did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    stop.store(true, Ordering::Relaxed);
    let _ = server.join();
    assert_eq!(status.code(), Some(4));
    assert!(!begin_seen.load(Ordering::SeqCst));
}

#[cfg(unix)]
fn run_validation_pty(args: &[&str], input: &[u8]) -> (Option<i32>, String) {
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::process::{Command as ProcessCommand, Stdio};
    use std::time::{Duration, Instant};

    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let mut command = ProcessCommand::new(assert_cmd::cargo::cargo_bin("neosense"));
    command
        .args(args)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()));
    let mut child = command.spawn().unwrap();
    drop(slave);
    master.write_all(input).unwrap();
    master.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "CLI did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    unsafe {
        libc::fcntl(master.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
    }
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
    (status.code(), String::from_utf8_lossy(&output).into_owned())
}

#[cfg(unix)]
#[test]
fn pty_safety_abort_prints_reason_and_recovery_and_exits_nonzero() {
    use neosense::ipc::{Request, Response, read_message, write_message};
    use neosense::validation::{
        ExpectedSide, RecoveryEvidence, StopReason, ValidationFan, ValidationLimits,
        ValidationPreflight, ValidationSession, ValidationState, ValidationStep,
        ValidationStepSpec, ValidationTelemetry,
    };
    use std::io::BufReader;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("neosensed.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let listener = Arc::new(listener);
    let stop = Arc::new(AtomicBool::new(false));
    let fan = ValidationFan {
        index: 0,
        backend_id: "nbfc:0".into(),
        name: "CPU fan".into(),
        role: neosense::policy::FanRole::Cpu,
    };
    let limits = ValidationLimits {
        step_duration_ms: 20,
        step_timeout_ms: 1000,
        session_ttl_ms: 10_000,
        ..ValidationLimits::default()
    };
    let mut session = ValidationSession::new(
        "abort-session".into(),
        fan.clone(),
        ExpectedSide::Left,
        limits.clone(),
        1,
        Some("test".into()),
        Some("0.5.4".into()),
    );
    session.state = ValidationState::Aborted;
    session.stop_reason = Some(StopReason::SafetyGuard(
        "selected fan 0 target/mode drifted during step 50%".into(),
    ));
    session.recovery = Some(RecoveryEvidence {
        attempted: true,
        backend_auto: true,
        ec_verified: Some(false),
        detail: "backend automatic; EC unverified".into(),
        completed_at_ms: 2,
    });
    let preflight = ValidationPreflight {
        schema: neosense::validation::PREFLIGHT_SCHEMA.into(),
        ready: true,
        blockers: Vec::new(),
        warnings: Vec::new(),
        fan: fan.clone(),
        expected_side: ExpectedSide::Left,
        other_fan: None,
        other_fan_auto: true,
        steps: ValidationStep::SEQUENCE
            .into_iter()
            .map(|step| ValidationStepSpec {
                step,
                requested_percent: step.percent(),
                duration_ms: limits.step_duration_ms,
            })
            .collect(),
        limits: limits.clone(),
        telemetry: ValidationTelemetry {
            collected_at_ms: 1,
            cpu: None,
            gpu: None,
            nbfc_fans: Vec::new(),
            ec_values: Vec::new(),
        },
        safety_state: Some("safe_auto".into()),
        selected_config: Some("test".into()),
        nbfc_version: Some("0.5.4".into()),
    };
    let server_stop = Arc::clone(&stop);
    let server_session = session.clone();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        while !server_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut reader = BufReader::new(&stream);
                    let request: Request = match read_message(&mut reader) {
                        Ok(request) => request,
                        Err(_) => continue,
                    };
                    let response = match request.method.as_str() {
                        "status.get" => Response::ok(
                            request.id,
                            serde_json::json!({
                                "health": {"healthy": true},
                                "nbfc": {
                                    "version": null, "read_only": false, "selected_config": "test",
                                    "fans": [
                                        {"index":0,"name":"CPU fan","temperature":55.0,"critical_temperature":95.0,"auto_mode":true,"critical":false,"current_speed":40.0,"target_speed":35.0,"requested_speed":50.0,"speed_steps":100},
                                        {"index":1,"name":"GPU fan","temperature":55.0,"critical_temperature":95.0,"auto_mode":true,"critical":false,"current_speed":40.0,"target_speed":35.0,"requested_speed":50.0,"speed_steps":100}
                                    ], "warnings": []
                                },
                                "sensors": {"sensors":[],"errors":[],"collected_at_ms":1},
                                "sensor_config": null, "service": {}, "last_error": null
                            }),
                        ),
                        "validation.preflight" => Response::ok(request.id, preflight.clone()),
                        "validation.begin" => Response::ok(request.id, server_session.clone()),
                        "validation.status" => Response::ok(
                            request.id,
                            serde_json::json!({"session": server_session, "last": null, "limits": limits}),
                        ),
                        "validation.cancel" | "validation.abort" => {
                            Response::ok(request.id, serde_json::json!({"aborted": false}))
                        }
                        _ => Response::error(
                            request.id,
                            neosense::ipc::ApiError::new("unknown_method", "unexpected method"),
                        ),
                    };
                    let _ = write_message(&mut stream, &response);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    let (code, output) = run_validation_pty(
        &[
            "fan",
            "validate",
            "cpu",
            "--socket",
            socket.to_str().unwrap(),
        ],
        b"yes\n",
    );
    stop.store(true, Ordering::Relaxed);
    let _ = server.join();
    assert_eq!(code, Some(6), "{output}");
    assert!(output.contains("VALIDATION ABORTED"), "{output}");
    assert!(output.contains("drifted during step 50%"), "{output}");
    assert!(output.contains("backend automatic=true"), "{output}");
    assert!(output.contains("EC verified=Some(false)"), "{output}");
    assert!(!output.contains("Recorded operator-attested"), "{output}");
    assert!(!output.contains("physical validation complete"), "{output}");
}

#[cfg(unix)]
#[test]
fn pty_cpu_then_gpu_continuation_requires_authorization_and_keeps_sessions_separate() {
    use neosense::ipc::{ApiError, Request, Response, read_message, write_message};
    use neosense::validation::{
        ExpectedSide, ObservationAnswer, RecoveryEvidence, ValidationFan, ValidationLimits,
        ValidationPreflight, ValidationSession, ValidationState, ValidationStep,
        ValidationStepSpec, ValidationTelemetry,
    };
    use std::io::BufReader;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("neosensed.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let listener = Arc::new(listener);
    let stop = Arc::new(AtomicBool::new(false));
    let limits = ValidationLimits {
        step_duration_ms: 20,
        step_timeout_ms: 1000,
        session_ttl_ms: 10_000,
        ..ValidationLimits::default()
    };
    let cpu = ValidationFan {
        index: 0,
        backend_id: "nbfc:0".into(),
        name: "CPU fan".into(),
        role: neosense::policy::FanRole::Cpu,
    };
    let gpu = ValidationFan {
        index: 1,
        backend_id: "nbfc:1".into(),
        name: "GPU fan".into(),
        role: neosense::policy::FanRole::Gpu,
    };
    let make_session = |id: &str, fan: ValidationFan, side: ExpectedSide| {
        let mut session = ValidationSession::new(
            id.into(),
            fan,
            side,
            limits.clone(),
            1,
            Some("test".into()),
            Some("0.5.4".into()),
        );
        session.state = ValidationState::AwaitingObservation;
        session.recovery = Some(RecoveryEvidence {
            attempted: true,
            backend_auto: true,
            ec_verified: Some(false),
            detail: "backend automatic; EC unverified".into(),
            completed_at_ms: 2,
        });
        session
    };
    let make_preflight = |fan: ValidationFan, side: ExpectedSide| ValidationPreflight {
        schema: neosense::validation::PREFLIGHT_SCHEMA.into(),
        ready: true,
        blockers: Vec::new(),
        warnings: Vec::new(),
        fan,
        expected_side: side,
        other_fan: None,
        other_fan_auto: true,
        steps: ValidationStep::SEQUENCE
            .into_iter()
            .map(|step| ValidationStepSpec {
                step,
                requested_percent: step.percent(),
                duration_ms: limits.step_duration_ms,
            })
            .collect(),
        limits: limits.clone(),
        telemetry: ValidationTelemetry {
            collected_at_ms: 1,
            cpu: None,
            gpu: None,
            nbfc_fans: Vec::new(),
            ec_values: Vec::new(),
        },
        safety_state: Some("safe_auto".into()),
        selected_config: Some("test".into()),
        nbfc_version: Some("0.5.4".into()),
    };
    let cpu_session = make_session("cpu-session", cpu.clone(), ExpectedSide::Left);
    let gpu_session = make_session("gpu-session", gpu.clone(), ExpectedSide::Right);
    let cpu_preflight = make_preflight(cpu.clone(), ExpectedSide::Left);
    let gpu_preflight = make_preflight(gpu.clone(), ExpectedSide::Right);
    struct ServerState {
        current: ValidationSession,
        continued: bool,
    }
    let state = Arc::new(Mutex::new(ServerState {
        current: cpu_session.clone(),
        continued: false,
    }));
    let server_stop = Arc::clone(&stop);
    let server_state = Arc::clone(&state);
    let server_limits = limits.clone();
    let server_cpu_session = cpu_session.clone();
    let server_gpu_session = gpu_session.clone();
    let server = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        while !server_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut reader = BufReader::new(&stream);
                    let request: Request = match read_message(&mut reader) {
                        Ok(request) => request,
                        Err(_) => continue,
                    };
                    let response = match request.method.as_str() {
                        "status.get" => Response::ok(
                            request.id,
                            serde_json::json!({
                                "health": {"healthy": true},
                                "nbfc": {
                                    "version": null, "read_only": false, "selected_config": "test",
                                    "fans": [
                                        {"index":0,"name":"CPU fan","temperature":55.0,"critical_temperature":95.0,"auto_mode":true,"critical":false,"current_speed":40.0,"target_speed":35.0,"requested_speed":50.0,"speed_steps":100},
                                        {"index":1,"name":"GPU fan","temperature":55.0,"critical_temperature":95.0,"auto_mode":true,"critical":false,"current_speed":40.0,"target_speed":35.0,"requested_speed":50.0,"speed_steps":100}
                                    ], "warnings": []
                                },
                                "sensors": {"sensors":[],"errors":[],"collected_at_ms":1},
                                "sensor_config": null, "service": {}, "last_error": null
                            }),
                        ),
                        "validation.preflight" => {
                            let fan = request.params["fan"].as_u64().unwrap_or(0);
                            if fan == 0 {
                                Response::ok(request.id, cpu_preflight.clone())
                            } else {
                                Response::ok(request.id, gpu_preflight.clone())
                            }
                        }
                        "validation.begin" => {
                            let fan = request.params["fan"].as_u64().unwrap_or(0);
                            let mut state = server_state.lock().unwrap();
                            if fan == 1 && !state.continued {
                                Response::error(
                                    request.id,
                                    ApiError::new("continuation_required", "GPU not authorized"),
                                )
                            } else {
                                state.current = if fan == 0 {
                                    server_cpu_session.clone()
                                } else {
                                    server_gpu_session.clone()
                                };
                                Response::ok(request.id, state.current.clone())
                            }
                        }
                        "validation.status" => {
                            let state = server_state.lock().unwrap();
                            Response::ok(
                                request.id,
                                serde_json::json!({"session": state.current, "last": null, "limits": server_limits}),
                            )
                        }
                        "validation.observe" => {
                            let mut state = server_state.lock().unwrap();
                            let answer = serde_json::from_str::<ObservationAnswer>(
                                request.params["answer"].as_str().unwrap_or("\"yes\""),
                            )
                            .unwrap_or(ObservationAnswer::Yes);
                            state.current.record_observation(answer, None, 3).unwrap();
                            Response::ok(request.id, state.current.clone())
                        }
                        "validation.continue" => {
                            let mut state = server_state.lock().unwrap();
                            state.continued =
                                state.current.fan.role == neosense::policy::FanRole::Cpu;
                            Response::ok(
                                request.id,
                                serde_json::json!({"continuation_authorized": state.continued}),
                            )
                        }
                        "validation.cancel" | "validation.abort" => {
                            Response::ok(request.id, serde_json::json!({"aborted": false}))
                        }
                        _ => Response::error(
                            request.id,
                            ApiError::new("unknown_method", "unexpected method"),
                        ),
                    };
                    let _ = write_message(&mut stream, &response);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    let (code, output) = run_validation_pty(
        &["fan", "validate", "--socket", socket.to_str().unwrap()],
        b"yes\nyes\nyes\nyes\nyes\n",
    );
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("CPU -> LEFT"), "{output}");
    assert!(output.contains("GPU -> RIGHT"), "{output}");
    assert_eq!(
        state.lock().unwrap().current.fan.role,
        neosense::policy::FanRole::Gpu
    );
    assert_eq!(state.lock().unwrap().current.id, "gpu-session");

    // Reset the fake supervisor and decline the CPU-to-GPU continuation.
    {
        let mut state = state.lock().unwrap();
        state.current = cpu_session.clone();
        state.continued = false;
    }
    let (code, output) = run_validation_pty(
        &["fan", "validate", "--socket", socket.to_str().unwrap()],
        b"yes\nyes\nno\n",
    );
    stop.store(true, Ordering::Relaxed);
    let _ = server.join();
    assert_eq!(code, Some(4), "{output}");
    assert!(output.contains("CPU -> LEFT"), "{output}");
    assert!(!output.contains("GPU -> RIGHT"), "{output}");
    assert!(!state.lock().unwrap().continued);
    assert_eq!(state.lock().unwrap().current.id, "cpu-session");
}
