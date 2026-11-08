# NeoSense — Implementation status (v2.3.0)

**Date:** 2026-11-08
**Scope:** Rust migration, supervisor hardening, TUI, profile compatibility and local validation.

## Architecture

- `neosense`: Rust CLI. Read commands use the core directly; control uses IPC.
- `neosensed`: Rust supervisor and sole fan-write authority.
- Core modules: typed sensors, hwmon/thermal/NVIDIA providers, NBFC backend, safety policy, profiles, warnings, diagnostics, IPC.
- `neosense tui`: Ratatui/Crossterm UI sharing the same core and IPC protocol.
- Shell: installer, uninstaller and systemd integration only.

## Resolved findings

| Finding | Status |
|---|---|
| Case-sensitive status parser | Resolved: exact NBFC text parser + JSON 0.5.4, fixtures |
| Invalid `manual` command | Resolved: supervisor-mediated control; `set -s` argument tested |
| Lowercase boolean handling | Resolved |
| Generic GPU `temp1` fallback | Resolved: typed GPU providers; Wi-Fi/NVMe/ACPI keep their roles |
| EC percentage treated as truth | Mitigated: requested/target/EC/RPM separated, warnings and diagnostics |
| Fixed dangerous presets | Removed: only `safe-auto` is applicable |
| Missing tests/CI | Resolved: 52 unit, 15 CLI, 2 installer tests |
| Version drift | Resolved: `Cargo.toml` is the single version source |
| Missing warning system | Resolved: shared severity-labelled warnings in CLI/TUI |
| Unvalidated profile catalogue | Resolved: compatibility evaluation and preview-only states |

## Profiles

Profile states: `compatible`, `compatible with warnings`, `preview only`, `unsupported`, `blocked for safety`.

- `safe-auto` is evaluated against NBFC availability, service state, CPU trust, GPU mapping, RPM and EC readback.
- `quiet`, `balanced`, `performance` and `custom` are preview-only: no closed-loop implementation or hardware validation.
- An unavailable profile cannot be applied and explains why.
- No profile may perform a fixed-speed write.

## Safety policy

Manual control is refused when:

- NBFC < 0.5.4;
- service is read-only or critical;
- CPU/GPU data is missing, invalid, stale or insufficiently trusted;
- the fan sensor source is unknown or the GPU fan is fed by `@CPU`;
- speed is below the floor or above 100%;
- temperature is above the ceiling;
- confirmation is absent.

Manual lease: 15 minutes. Lease expiry, critical NBFC state, lost sensors, failed verification or backend loss trigger automatic recovery or fail-safe state.

## IPC and permissions

- Socket: `/run/neosense/neosensed.sock`, `root:neosense`, mode `0660`.
- Runtime directory: `/run/neosense`, `root:neosense`, mode `0750`.
- Group: `neosense`, created by the installer; membership requires a new login session.
- The user outside the group receives exit code 4 and a group/login hint.
- **NBFC's socket remains `0666`**: the group protects NeoSense only; it is not a complete security boundary for fan control.

## systemd

- `Type=notify`, `NotifyAccess=main`, `WatchdogSec=10s`.
- `READY=1` after socket bind, permissions and initial state snapshot.
- `WATCHDOG=1` only while health checks pass.
- `STOPPING=1` on orderly shutdown.
- `ExecStopPost=neosensed --fail-safe`, which sets NBFC automatic mode and verifies the result.
- User-systemd transient tests with a fake NBFC backend validated READY, watchdog, stop, restart after SIGKILL and ExecStopPost.
- Root-level `/etc/systemd/system` service and real `neosense` group membership were **not** validated because sudo required a password not available in the session.

## TUI

Six views: Dashboard, Sensors, Fans, Profiles, System, Diagnostics. Profile selection uses Up/Down and Enter; applying `safe-auto` requires `y` confirmation and goes through IPC. No manual speed is exposed. Without a supervisor, the TUI is read-only.

Rendering tests cover 40×20, 80×24, 100×32 and 160×48. A pty 100×32 smoke test was performed; a human visual review remains.

## Local validation

Confirmed: Acer Nitro AN515-57, BIOS V1.17, i5-11400H, RTX 3050 Laptop, Nobara 44, kernel 7.3.0, NBFC 0.5.3.

- CPU: coretemp package/cores; ACPI and x86_pkg thermal zones.
- GPU: NVIDIA via `nvidia-smi`; memory temperature N/A correctly unavailable.
- NVMe and Wi-Fi classified separately; Wi-Fi never promoted to GPU.
- NBFC: two fans, both use `@CPU`; GPU fan is not driven by GPU temperature.
- EC reports 100% against varying targets; no RPM; warnings visible in CLI/TUI.
- CLI status/sensors/watch/diagnose/service/hardware/profile/TUI tested read-only.
- `hardware validate` returns code 4 because `rate-config` needs root.
- `service status` returns code 4 for a user outside the group.

## Test commands

```text
cargo fmt --all -- --check                  OK
cargo clippy --all-targets --all-features -- -D warnings  OK
cargo test --all-targets --all-features    OK (52 unit, 15 CLI, 2 installer)
cargo build --release --locked             OK
shellcheck install.sh uninstall.sh         OK
bash -n install.sh uninstall.sh            OK
systemd-analyze verify (neutralized unit)  OK
```

No test writes fan state. The supervisor/TUI tests use fake backends or temporary sockets.

## Remaining work

- Root-level installation, group membership and service lifecycle validation (sudo blocked).
- Real NBFC ≥ 0.5.4 manual-control test with RPM and explicit user authorization.
- Closed-loop profiles, threshold/hysteresis implementation and hardware matrix.
- `fan release`/`reset-ec` validation.
- Human visual review of the TUI.
- NBFC socket `0666` remains an upstream limitation.
