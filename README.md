# NeoSense

**Thermal monitoring and safe fan management for Linux laptops.**

NeoSense is a modern interface around **NBFC (Notebook FanControl)**. It is written in Rust and keeps fan writes behind a single safety supervisor.

> NeoSense is not a replacement for NBFC. NBFC remains responsible for the embedded controller (EC), hardware configurations and vendor critical-temperature handling.

## Project status

- **Version:** 2.4.0 development (Rust).
- **Release status:** 2.3.0 remains an unpublished frozen artifact; 2.4.0 is in development as a fan-control application with supervisor-owned closed-loop profiles. Neither is publicly released.
- **Fan backend:** NBFC.
- **Platform:** Linux.
- **TUI:** Ratatui/Crossterm primary interface for fan discovery, per-fan manual control, automatic mode, profiles and diagnostics; every write goes through the supervisor with confirmation.
- **Thermal profiles:** built-in and persistent custom curves can be applied to policy-eligible fans by the supervisor; blocked fans remain automatic. Physical identity, calibration and actuator response remain unvalidated.
- **Manual fan control:** index-based and supervisor-owned, with a bounded target, strict lease timeout and automatic recovery. The GPU logical channel can use an independent, trusted NVIDIA GPU temperature as its guard even when NBFC declares `@CPU`; this is reported as a warning and does not establish physical fan identity.

## Commands

| Command | Purpose |
|---|---|
| `neosense auto` | Restore all fans to NBFC automatic mode (v1.0.1 compatibility) |
| `neosense manual` | Enter manual mode at the current targets (v1.0.1 compatibility) |
| `neosense set cpu\|gpu\|both <percent>` | Set fixed logical fan targets through the supervisor |
| `neosense turbo\|performance\|balanced\|silent` | Apply the historical fixed presets |
| `neosense status` | Concise CPU/GPU, backend, fan and safety summary |
| `neosense status --json` | Versioned JSON summary |
| `neosense sensors list` | Typed sensor inventory with provenance and trust |
| `neosense sensors show <id>` | Raw and normalized details for one sensor |
| `neosense sensors watch` | Read-only periodic refresh |
| `neosense fans list` | Discovered logical fan inventory with identity/source/capability |
| `neosense fans show <index>` | Detailed state, blockers and feedback for one fan |
| `neosense set fan <index> <percent>` | Index-based fixed target, subject to safety policy |
| `neosense auto fan <index>` | Return one logical fan to automatic mode |
| `neosense fan status` | Fan mode, requested/target/EC values and source |
| `neosense fan auto` | Return to NBFC automatic mode via the supervisor |
| `neosense fan set` | Manual speed request, subject to safety policy |
| `neosense fan hold` | Hold current target, subject to safety policy |
| `neosense fan release` | Explicit firmware/EC release (not implemented without validation) |
| `neosense fan validate` | Interactive CPU→LEFT then GPU→RIGHT physical observation workflow; requires a TTY, explicit yes/no authorizations and separately authorized execution |
| `neosense validation history` | Read-only export of supervisor-owned validation evidence |
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
| `neosense calibration analyze` | Offline P1 regression/uncertainty analysis of paired RPM samples |
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
- Fan Control: Up/Down selects a discovered fan; `m` enters a numeric target; `+`/`-` adjusts by 5%; `a` returns the selected fan to automatic; `A` returns all fans to automatic. Every operation asks `y`/`n` and uses supervisor IPC.
- Profiles: Up/Down selects; the selected curve is shown as compact percentage bars; Enter previews/simulates (safe-auto has no curve and is applied with `a`); `a` applies the profile to eligible fans through the supervisor; `d` deactivates the active profile and returns fans to automatic; `e` edits a custom curve in `$EDITOR`; `n` creates a new custom curve. Blocked fans are returned to automatic and reported.
- Fan Control `a` returns the selected fan to automatic; if a profile is active, it continues regulating the remaining applied fans. `A` returns all fans to automatic and ends the profile.
- Presets: Up/Down select a historical fixed target; Enter asks `y`/`n`. Fixed presets use the same supervisor safety gate and are blocked on this machine.
- `A`: return all fans to NBFC automatic mode, with confirmation.
- The Dashboard shows the supervisor safety state and any recovery failure. Without a supervisor the TUI stays read-only and says so.
- See [`docs/TUI.md`](docs/TUI.md) for navigation, rendering evidence and profile semantics.

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
7. **Profiles are validated and applied per fan.** `safe-auto` returns to NBFC automatic mode. Quiet/Balanced/Performance and custom curves are applied by the supervisor only to fans that pass mapping, temperature and backend policy; blocked fans remain automatic and are reported. Physical identity, calibration and actuator-response validation remain separate.
8. **Warnings are explicit.** Every warning carries a severity label, cause and next safe action; colour is never the only signal.

### Known limitations

- **Optional NBFC analysis dependency.** `neosense hardware validate` runs the
  read-only NBFC `rate-config` analysis through the supervisor. It requires
  NBFC's optional `acpiexec` (`acpica-tools`); when absent, the command reports
  a clear `analysis_unavailable` limitation and performs no fan write.
- **NBFC socket is root-only.** The host runs the reviewed NBFC 0.5.4 patch: `/run/nbfc_service.socket` is `0600 root:root`, `/etc/nbfc/access.json` is root-owned and only UID 0 may connect. An ordinary user's `nbfc status` is expected to fail at connect time; the CLI/TUI never access NBFC directly and read backend state from the supervisor over IPC. Unpatched upstream NBFC still hardcodes `chmod 0666`.
- **Readback is a clamp artifact and a config omission.** With `IndependentReadMinMaxValues=false`, NBFC 0.5.4 compares raw 16-bit EC words against the 0..100 write range and clamps them, so `CurrentSpeed=100%` does not mean a fan runs at 100%. Thirteen sibling Acer Nitro/Predator configs declare an independent read scale (max 5263–7317) for the same `19/55` and `21/58` registers; the AN515-57 config does not. The unit and true range remain unproven. Per-fan control and profile application are policy-gated. The GPU logical channel is eligible when a fresh, trusted independent NVIDIA GPU temperature is available; NBFC's `@CPU` source is not used as a GPU thermal substitute. Physical identity and actuator response remain unverified.
- **Acer tachometer candidate is provisional.** The running `acer_wmi` driver has no Acer hwmon fan input on this kernel. Read-only EC bytes `0x13/0x14` and `0x15/0x16` produce plausible, stable candidate values (`3896` and `5263` in the audit), consistent with the community `acer-nitro-ec` mapping. That driver assembles them as `(low << 8) | high` and does not convert units. NeoSense exposes them only as provisional, unmapped candidates: they never enter regulation and are not accepted as validated RPM feedback.
- **Physical validation is operator-attested, not RPM calibration.** `neosense fan validate` runs a supervisor-owned 0%/50%/100% sequence for five seconds per step, recovers to automatic before asking whether the expected physical behavior was observed, and records append-only evidence. The workflow requires separate authorizations for CPU and GPU. A 0% target does not prove physical stoppage and a 100% target does not prove maximum RPM. Ordinary manual control still has a 20% floor; validation 0% uses a separate session-bound gate. The workflow remains unauthorized to execute on this machine.
- **NBFC automatic is not EC automatic.** NBFC `AutoMode=true` describes its internal software curve. The installed Acer config sets manual EC modes (`0x22=0x0c`, `0x21=0x30`) and NBFC writes the target registers, so automatic fan reporting and manual EC bytes can coexist legitimately. NeoSense reports raw EC mode evidence separately and marks EC automatic mode unverified unless the raw mode matches the community auto code.
- **Software, backend and physical validation are separate.** `diagnose` reports RPM available/fresh/trusted, fan identity and physical-control validation independently. A clean diagnostics report is not physical fan control.
- The selected NBFC configuration exposes separate CPU/GPU register pairs, but both logical fans use `@CPU`. The GPU requires the independent NVIDIA temperature guard; physical fan identity and the EC channel-to-fan mapping remain unverified.
- RPM telemetry is feedback only. It never feeds temperature regulation and does not by itself enable manual/profile writes.
- Some NBFC configurations drive the GPU fan from `@CPU`; NeoSense reports this and can use an independent NVIDIA GPU temperature guard for the GPU logical channel. It never substitutes CPU temperature and never claims physical identity.
- Quiet/Balanced/Performance and persistent custom curves are supervisor-applied to eligible fans, with partial application and blocked-fan reporting. Persistent user curves use strict validation, atomic writes, backups and corruption recovery.
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
- TUI fan control, profile catalogue, supervisor closed-loop application, persistent custom curves and compatibility checks (done).
- Physical validation of the now-implemented closed-loop/manual paths, a working NBFC GPU source and full hardware matrix (next).

## License

MIT. See [`LICENSE`](LICENSE).
