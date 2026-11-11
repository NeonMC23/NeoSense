# NeoSense

**Thermal monitoring and safe fan management for Linux laptops.**

NeoSense is a modern interface around **NBFC (Notebook FanControl)**. It is written in Rust and keeps fan writes behind a single safety supervisor.

> NeoSense is not a replacement for NBFC. NBFC remains responsible for the embedded controller (EC), hardware configurations and vendor critical-temperature handling.

## Project status

- **Version:** 2.4.0 development (Rust).
- **Release status:** 2.3.0 remains an unpublished frozen artifact; 2.4.0 is in development for the profile simulation engine. Neither is publicly released.
- **Fan backend:** NBFC.
- **Platform:** Linux.
- **TUI:** Ratatui/Crossterm, read-only by default; `safe-auto` can be applied through the supervisor with confirmation.
- **Thermal profiles:** deterministic preview/simulation engine with persistent custom curves; real closed-loop fan application remains disabled by hardware-safety policy.
- **Manual fan control:** implemented but **blocked by policy** on NBFC < 0.5.4, missing/stale sensors, ambiguous fan mapping and unreliable readback.

## Commands

| Command | Purpose |
|---|---|
| `neosense status` | Concise CPU/GPU, backend, fan and safety summary |
| `neosense status --json` | Versioned JSON summary |
| `neosense sensors list` | Typed sensor inventory with provenance and trust |
| `neosense sensors show <id>` | Raw and normalized details for one sensor |
| `neosense sensors watch` | Read-only periodic refresh |
| `neosense fan status` | Fan mode, requested/target/EC values and source |
| `neosense fan auto` | Return to NBFC automatic mode via the supervisor |
| `neosense fan set` | Manual speed request, subject to safety policy |
| `neosense fan hold` | Hold current target, subject to safety policy |
| `neosense fan release` | Explicit firmware/EC release (not implemented without validation) |
| `neosense profile list/show/apply` | Profile catalogue and compatibility; only `safe-auto` is applicable |
| `neosense profile preview <name>` | Calculate reference fan targets locally with no write |
| `neosense profile select/state/clear` | Select, inspect or clear a 2.4.0 supervisor simulation profile; no fan write |
| `neosense profile custom ...` | Create/list/show/update/duplicate/rename/delete/validate/edit persistent user curves |
| `neosense preset list/show/apply` | Historical fixed targets: Silent 25%, Balanced 60%, Performance 80%, Turbo 100%, policy-gated |
| `neosense hardware list/current/validate` | NBFC model configurations and `rate-config` analysis |
| `neosense hardware identity` | Read-only per-fan register, sensor-source and readback-capability evidence |
| `neosense service status` | NBFC and NeoSense supervisor status |
| `neosense diagnose` | Read-only compatibility and safety diagnostics |
| `neosense control readiness` | Explain control eligibility by requirement and evidence |
| `neosense config show/path` | Effective settings and supervisor socket |
| `neosense tui` | Interactive terminal UI |
| `neosense completions <shell>` | Generate bash/zsh/fish/elvish/powershell completions |
| `neosense version` | Version |

Live sensor and backend read commands require `neosensed`; the CLI never reads hardware or NBFC directly. All state-changing operations go through the supervisor.

## Installation

### Platform support

NeoSense targets Linux laptops with NBFC, systemd and readable hwmon/thermal
sensors. The current release is built and tested for
`x86_64-unknown-linux-gnu`; no broad distribution or laptop compatibility claim
is made from the single development machine. The tested environment is Nobara
44 with kernel 7.3 and NBFC 0.5.3. Other distributions may work, but should be
validated read-only first with `neosense diagnose --json`.

### Requirements

- Linux with systemd and NBFC installed and configured.
- **NBFC 0.5.4 or newer recommended** for protected manual control (`status --json`, `reset-ec`, systemd `ExecStopPost`).
- NBFC 0.5.3 remains usable read-only and for `fan auto`/`diagnose`.
- Rust/Cargo only for a source install.
- `sudo` for installing binaries and the systemd unit.
- No Python or other runtime dependency from NeoSense itself.

### From a release archive (recommended)

Download `neosense-<version>-<target>.tar.gz` and `SHA256SUMS`, verify the
checksum, then install:

```bash
sha256sum -c SHA256SUMS
tar -xzf neosense-2.4.0-x86_64-unknown-linux-gnu.tar.gz
cd neosense-2.4.0-x86_64-unknown-linux-gnu
sudo ./install.sh
```

The archive contains prebuilt binaries, so Rust is not required. The installer
uses `bin/neosense` and `bin/neosensed` when present.

### From source

```bash
git clone git@github.com:NeonMC23/NeoSense.git
cd NeoSense
sudo ./install.sh
```

A source checkout has no `bin/` directory, so the installer builds release
binaries with Cargo. Both paths create the `neosense` system group if needed and
install:

- `/usr/local/bin/neosense`
- `/usr/local/bin/neosensed`
- `/etc/systemd/system/neosensed.service`
- `/run/neosense/` (`root:neosense`, mode `0750`)

It does not start an inactive supervisor. If the supervisor was already
running, the installer restarts and verifies it after a successful upgrade;
`--no-restart` keeps the old process running and prints the explicit restart
step. A running supervisor's designed shutdown fail-safe may return NBFC to
automatic mode, so this is not described as a zero-write upgrade.

Before replacing files, the installer stages and verifies the built binary, creates a timestamped backup under `${NEOSENSE_STATE_DIR:-/var/lib/neosense}/backups/`, and restores the previous binary/unit if installation verification fails. It supports an in-place upgrade and verifies the restarted supervisor's IPC state. If restart verification fails, it restores the previous binaries/unit and attempts to restart the previous supervisor.

### Supervisor access

Add an authorized user to the `neosense` group:

```bash
sudo usermod -aG neosense "$USER"
# Log out and back in, or run: newgrp neosense
```

The IPC socket is created as `root:neosense`, mode `0660`; the runtime directory is `root:neosense`, mode `0750`. A user outside the group receives a clear permission error (exit code 4).

### Real-host checklist

See [`docs/HOST_INSTALL.md`](docs/HOST_INSTALL.md) for the root installation,
group-session, service, rollback and uninstall checklist.

### Enable the supervisor

```bash
sudo systemctl enable --now neosensed
```

The supervisor is the sole fan-write authority. It sends `READY=1` only after the socket is bound, permissioned, and an initial state snapshot is available. It sends `WATCHDOG=1` only while its health checks pass, and `STOPPING=1` during orderly shutdown.

The service uses `Type=notify` and `WatchdogSec=10s`. See [`docs/SYSTEMD.md`](docs/SYSTEMD.md).

### Release archive

```bash
./packaging/build-release.sh
```

This produces `dist/neosense-<version>-<target>.tar.gz` containing the binaries, installer, uninstaller, systemd unit, docs, completions, `BUILD-INFO` and checksums. The archive is generated locally and is not committed. Validate it before publishing:

```bash
./packaging/test-release-archive.sh dist/neosense-*.tar.gz
```

The test checks archive contents and, when user namespaces are available,
installs and uninstalls the extracted prebuilt archive in a temporary root.

### Uninstall

```bash
sudo ./uninstall.sh
```

The script stops the supervisor (triggering automatic recovery), removes NeoSense-owned binaries, unit, runtime/state directories and backups, and removes the `neosense` group only when it is empty and has no primary users. NBFC configuration is untouched.

## Usage

```bash
neosense status
neosense sensors list
neosense sensors show hwmon:coretemp:...:temp1
neosense fan status
neosense diagnose
neosense hardware list
neosense hardware current
sudo neosense hardware validate "Acer Nitro AN515-57 Custom"
```

`hardware validate` calls `nbfc rate-config`, which needs root to read ACPI tables.

Control through the supervisor:

```bash
sudo systemctl start neosensed
neosense fan auto
neosense fan set cpu 50 --yes
```

After group membership and a new login session, control commands no longer need `sudo`. `fan set` exits 8 if the safety policy cannot establish a safe operation. On NBFC 0.5.3 manual control is deliberately refused.

## TUI

```bash
neosense tui
```

The header shows the application/version, hardware model, NBFC version and a compact supervisor state. Full IPC failures live in System/Diagnostics. Seven views: Dashboard, Sensors, Fans, Profiles, Presets, System and Diagnostics.

- `Tab`/arrows: change view; `1`–`7`: direct access (`7` opens fixed Presets).
- `r`: refresh; `?`: help; `q`/Esc: quit.
- Profiles: Up/Down select; Enter previews/simulates the selected curve; `y` confirms, `n` cancels. Closed-loop and custom profiles are simulation-only and never write fan state. `e` edits the selected custom curve in `$EDITOR`; `n` creates a new custom curve.
- Presets: Up/Down select a historical fixed target; Enter asks `y`/`n`. Fixed presets use the same supervisor safety gate and are blocked on this machine.
- `a`: return to NBFC automatic mode, with confirmation.
- Without a supervisor the TUI stays read-only and says so.
- No direct manual-speed path exists in the TUI. See [`docs/TUI.md`](docs/TUI.md) for the real-terminal rendering evidence.

## Diagnostics

`neosense diagnose` is grouped for scanning:

1. overall summary (operational, supervisor, fan control, thermal reliability, critical/warning counts);
2. critical blockers;
3. backend;
4. hardware;
5. thermal monitoring;
6. fan telemetry;
7. control capabilities;
8. security and permissions.

`neosense diagnose` also reports the structured control stage (`unavailable`, `hardware detected`, `hardware eligible`, `hardware unvalidated`, `hardware validated`), per-fan eligibility and explicit blockers. `neosense control readiness --json` exposes the same assessment directly. `diagnose --json` includes stable groups, a machine-readable summary and the full `control` object. See [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md).

## Safety

1. **Single writer.** Only `neosensed` writes fan state.
2. **No invented values.** Missing, invalid, stale or unreadable sensors are `null`/`unavailable`, never `0 °C`.
3. **Evidence before control.** Manual control needs valid, fresh and trusted CPU/GPU data, a known fan mapping, a floor, a ceiling and explicit confirmation.
4. **No generic GPU sensor.** Wi-Fi/NVMe/ACPI channels are never promoted to GPU by keyword.
5. **No false success.** `nbfc set` acceptance is not proof of physical fan speed. EC readback is not RPM.
6. **Fail safe.** Lease expiry, lost sensors, NBFC critical mode or failed verification trigger automatic recovery or a fail-safe state.
7. **Profiles are validated.** `safe-auto` is compatible only when NBFC, service and CPU evidence are available. Closed-loop Quiet/Balanced/Performance profiles use deterministic reference curves for preview/simulation; they are not applied to hardware until mapping, feedback, recovery and authorization are validated.
8. **Warnings are explicit.** Every warning carries a severity label, cause and next safe action; colour is never the only signal.

### Known limitations

- **NBFC socket is root-only.** The host runs the reviewed NBFC 0.5.4 patch: `/run/nbfc_service.socket` is `0600 root:root`, `/etc/nbfc/access.json` is root-owned and only UID 0 may connect. An ordinary user's `nbfc status` is expected to fail at connect time; the CLI/TUI never access NBFC directly and read backend state from the supervisor over IPC. Unpatched upstream NBFC still hardcodes `chmod 0666`.
- **Readback is a clamp artifact and a config omission.** With `IndependentReadMinMaxValues=false`, NBFC 0.5.4 compares raw 16-bit EC words against the 0..100 write range and clamps them, so `CurrentSpeed=100%` does not mean a fan runs at 100%. Thirteen sibling Acer Nitro/Predator configs declare an independent read scale (max 5263–7317) for the same `19/55` and `21/58` registers; the AN515-57 config does not. The unit and true range remain unproven, so manual/preset/closed-loop hardware operations stay refused.
- **Acer RPM feedback is available read-only.** The audited `acer_wmi` candidate exposes `hwmon6` `name=acer` with `fan1_input`/`fan2_input` (`0444`), no PWM and no `platform_profile`. NeoSense labels them neutrally as `Acer Fan 1`/`Acer Fan 2` and marks them `physical_measurement=true`, `usable_for_feedback=true` when fresh/trusted, and `usable_for_regulation=false`. Observed values include `3488–4918 rpm` and `4918–6382 rpm`; CPU/GPU identity and calibration are unverified.
- **Software, backend and physical validation are separate.** `diagnose` reports RPM available/fresh/trusted, fan identity and physical-control validation independently. A clean diagnostics report is not physical fan control.
- The selected NBFC configuration exposes separate CPU/GPU register pairs, but both logical fans use `@CPU`; the Acer RPM channels have no labels. Independent physical fan identity remains unverified.
- RPM telemetry is feedback only. It never feeds temperature regulation and does not enable manual or automatic hardware writes.
- Some NBFC configurations drive the GPU fan from `@CPU`; `diagnose` reports this and GPU manual control is blocked.
- Closed-loop Quiet/Balanced/Performance profiles now have deterministic simulation curves but are not applied to hardware. Persistent user curves are supported with strict validation, atomic writes, backups and corruption recovery.
- `neosense hardware identity` reads the selected NBFC configuration without modifying it and reports registers, sensor sources, readback validity and identity limits. See [`docs/PROFILES.md`](docs/PROFILES.md) and [`docs/CUSTOM_PROFILES.md`](docs/CUSTOM_PROFILES.md).
- `fan release` is intentionally unavailable until `reset-ec`/firmware return is validated.
- No broad laptop compatibility claim is made from a single machine.

## Architecture

```text
neosense (Rust CLI) ── IPC ──┐
neosense tui (Ratatui) ─────┴── Unix JSON ──► neosensed
                                             ├── sensor registry / hwmon / thermal / NVIDIA
                                             ├── NBFC backend
                                             ├── safety policy + verification
                                             └── systemd READY / WATCHDOG / STOPPING
```

- `neosense status/diagnose/fan status/sensors` read backend state **and the sensor snapshot** from the supervisor over IPC and never write. Offline NBFC configuration files are the only local data read by the CLI; live hardware sensors are never collected in the client process.
- `fan`/`profile` writes and TUI controls use the single supervisor IPC protocol.
- The CLI/TUI never connect to the NBFC socket; the root supervisor is the only NBFC client.
- `neosensed` owns fan writes and recovery.
- Shell is limited to installation, removal and systemd integration.

## Project tracking and release

- [`docs/CLI.md`](docs/CLI.md) is the complete CLI reference.
- [`docs/CLI_AUDIT.md`](docs/CLI_AUDIT.md) records the command inventory and validation matrix.
- [`docs/HARDWARE.md`](docs/HARDWARE.md) records current hardware evidence and backend acceptance criteria.
- [`docs/PROFILES.md`](docs/PROFILES.md) documents curves, simulation boundaries and hardware-control prerequisites.
- [`docs/HISTORICAL.md`](docs/HISTORICAL.md) compares the v1.0.1 feature set with the modern implementation.
- [`docs/CUSTOM_PROFILES.md`](docs/CUSTOM_PROFILES.md) documents the persistent curve schema, editing, validation and recovery.
- [`docs/NBFC_ACCESS_CONTROL.md`](docs/NBFC_ACCESS_CONTROL.md) is a technical proposal for upstream NBFC socket permissions and peer-credential authorization.
- [`docs/ROADMAP.md`](docs/ROADMAP.md) tracks feature status, evidence and blockers.
- [`docs/RELEASE.md`](docs/RELEASE.md) describes the release checklist and artifacts.
- [`docs/RELEASE_NOTES.md`](docs/RELEASE_NOTES.md) contains the frozen 2.3.0 notes.
- [`docs/RELEASE_NOTES_2.4.0.md`](docs/RELEASE_NOTES_2.4.0.md) contains the 2.4.0 development notes.
- [`CHANGELOG.md`](CHANGELOG.md) records user-visible changes.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`SECURITY.md`](SECURITY.md) cover contribution and disclosure.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release --locked
shellcheck install.sh uninstall.sh
```

Tests never write fan state. Unit tests cover NBFC parsing/configuration identity, sensor fixtures, NVIDIA parsing, policy, built-in and custom profiles, storage recovery, IPC, daemon lifecycle and TUI rendering. See [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md).

### Hardware testing

Hardware-dependent tests are separate. `neosense diagnose --json` is the read-only entry point. No fan speed, EC reset, firmware or NBFC configuration change should be performed without explicit authorization and a validated recovery plan.

## Compatibility

| Item | Status |
|---|---|
| Acer Nitro AN515-57, i5-11400H, RTX 3050, NBFC 0.5.3 | Read-only verified; manual control refused |
| Other laptops | Must be validated individually; no broad claim |
| NBFC 0.5.4+ | Recommended for protected manual control; test on target hardware |
| iGPU / no GPU | No invented GPU temperature; GPU control unavailable |
| No RPM | Verification limited; reported by `diagnose` |

## Roadmap

- Sensor and CLI correctness (done).
- Typed sensor model and read-only diagnostics (done).
- Supervisor, IPC and systemd recovery foundations (done).
- TUI, profile catalogue, deterministic simulation engine, persistent custom curves and compatibility checks (done).
- Real closed-loop hardware application, full hardware matrix and root-level systemd validation (next).

## License

MIT. See [`LICENSE`](LICENSE).
