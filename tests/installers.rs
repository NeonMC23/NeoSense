use std::process::Command;
use tempfile::tempdir;

fn run(
    script: &str,
    args: &[&str],
    prefix: &std::path::Path,
    systemd: &std::path::Path,
    runtime: &std::path::Path,
    state: &std::path::Path,
) -> std::process::Output {
    Command::new("bash")
        .arg(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), script))
        .args(args)
        .env("NEOSENSE_PREFIX", prefix)
        .env("NEOSENSE_SYSTEMD_DIR", systemd)
        .env("NEOSENSE_RUNTIME_DIR", runtime)
        .env("NEOSENSE_STATE_DIR", state)
        .output()
        .unwrap()
}

#[test]
fn install_and_uninstall_dry_runs_are_safe_and_idempotent() {
    let tmp = tempdir().unwrap();
    let prefix = tmp.path().join("prefix");
    let systemd = tmp.path().join("systemd");
    let runtime = tmp.path().join("run");
    let state = tmp.path().join("state");

    let install = run(
        "install.sh",
        &["--dry-run"],
        &prefix,
        &systemd,
        &runtime,
        &state,
    );
    assert!(install.status.success(), "{install:?}");
    let text = String::from_utf8_lossy(&install.stdout);
    assert!(text.contains("target/release/neosense"));
    assert!(text.contains("target/release/neosensed"));
    assert!(!prefix.exists());
    assert!(!systemd.exists());
    assert!(!runtime.exists());
    assert!(!state.exists());

    for _ in 0..2 {
        let uninstall = run(
            "uninstall.sh",
            &["--dry-run"],
            &prefix,
            &systemd,
            &runtime,
            &state,
        );
        assert!(uninstall.status.success(), "{uninstall:?}");
        assert!(
            String::from_utf8_lossy(&uninstall.stdout).contains("NeoSense has been uninstalled")
        );
    }
}

#[test]
fn unsafe_paths_are_rejected_by_both_scripts() {
    let tmp = tempdir().unwrap();
    let prefix = tmp.path().join("prefix");
    let systemd = tmp.path().join("systemd");
    let runtime = tmp.path().join("run");
    let state = tmp.path().join("state");

    for script in ["install.sh", "uninstall.sh"] {
        for arg in ["--prefix=/", "--runtime-dir=/", "--state-dir=/tmp/../etc"] {
            let output = run(
                script,
                &["--dry-run", arg],
                &prefix,
                &systemd,
                &runtime,
                &state,
            );
            assert_eq!(output.status.code(), Some(2), "{script} {arg}: {output:?}");
        }
    }
}
#[test]
fn invalid_group_is_rejected_before_any_action() {
    let tmp = tempdir().unwrap();
    let output = run(
        "install.sh",
        &["--dry-run", "--group=bad;name"],
        &tmp.path().join("prefix"),
        &tmp.path().join("systemd"),
        &tmp.path().join("run"),
        &tmp.path().join("state"),
    );
    assert_eq!(output.status.code(), Some(2));
}
