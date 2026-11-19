# NeoSense

**Safe, observable fan control for Linux laptops — one supervisor, a clear terminal UI and a scriptable CLI.**

[![Release](https://img.shields.io/github/v/release/NeonMC23/NeoSense?label=release)](https://github.com/NeonMC23/NeoSense/releases/latest)
[![CI](https://github.com/NeonMC23/NeoSense/actions/workflows/ci.yml/badge.svg)](https://github.com/NeonMC23/NeoSense/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/NeonMC23/NeoSense)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Linux-blue)

NeoSense is a Rust application that puts a practical, safety-first interface on
top of [NBFC (Notebook FanControl)](https://github.com/hirschmann/nbfc). It adds
automatic, preset and manual fan modes, temperature-driven profiles, a Ratatui
terminal UI and structured JSON output — while keeping every fan write behind a
single supervised backend with leases, verification and automatic recovery.

> **Relationship to NBFC.** NeoSense does not replace NBFC. NBFC still owns the
> embedded controller, the laptop configuration and the vendor critical-temperature
> behavior. NeoSense owns the user workflow and the safety policy in front of it.

**Stable release:** [v2.4.0](https://github.com/NeonMC23/NeoSense/releases/tag/v2.4.0)
(published 2026-11-19). Linux only; there are no Windows or macOS builds.

```text
███╗   ██╗ ███████╗  ██████╗  ███████╗ ███████╗ ███╗   ██╗ ███████╗ ███████╗
████╗  ██║ ██╔════╝ ██╔═══██╗ ██╔════╝ ██╔════╝ ████╗  ██║ ██╔════╝ ██╔════╝
██╔██╗ ██║ █████╗   ██║   ██║ ███████╗ █████╗   ██╔██╗ ██║ ███████╗ █████╗
██║╚██╗██║ ██╔══╝   ██║   ██║ ╚════██║ ██╔══╝   ██║╚██╗██║ ╚════██║ ██╔══╝
██║ ╚████║ ███████╗ ╚██████╔╝ ███████║ ███████╗ ██║ ╚████║ ███████║ ███████╗
╚═╝  ╚═══╝ ╚══════╝  ╚═════╝  ╚══════╝ ╚══════╝ ╚═╝  ╚═══╝ ╚══════╝ ╚══════╝
```

---

## Contents

- [Why NeoSense](#why-neosense)
- [How it works](#how-it-works)
- [Capabilities](#capabilities)
- [Install](#install)
- [First run](#first-run)
- [Everyday use](#everyday-use)
- [Terminal UI](#terminal-ui)
- [Safety model](#safety-model)
- [Diagnostics](#diagnostics)
- [Supported platforms](#supported-platforms)
- [Known limitations](#known-limitations)
- [Documentation](#documentation)
- [Development](#development)

---

## Why NeoSense

- **Everyday control in one command.** `neosense silent`, `balanced`, `maximum`
  or `auto` — or pick a mode from the TUI Dashboard.
- **One writer.** Only the root `neosensed` supervisor talks to NBFC and
  hardware. The CLI and TUI use a local JSON protocol and are read-only when the
  supervisor is unavailable.
- **Honest telemetry.** Requested target, backend-accepted target, EC readback
  and physical response are separate concepts. Missing or stale sensors are
  `unavailable`, never `0 °C`; EC percentages are never presented as RPM.
- **Fail-safe by design.** Fixed targets are leases. Lease expiry, stale
  telemetry, NBFC critical mode, failed verification or a supervisor crash
  returns fans to NBFC automatic mode. A failed recovery is reported and
  retried a bounded number of times.
- **Policy-gated profiles.** Quiet, Balanced and Performance curves are applied
  per fan only when mapping, temperatures and backend guarantees allow it;
  blocked fans stay automatic and are reported with reasons.
- **One install entry point.** `neosense.sh` detects the distribution, checks
  versioned dependencies, reports backend/controller conflicts and verifies the
  result. It never rewrites NBFC configuration or stops another controller.

## How it works

```text
 neosense (CLI)  ──┐
 neosense tui     ─┴── Unix JSON IPC ──► neosensed (root, sole writer)
                                            ├── sensor registry (hwmon/thermal/NVIDIA)
                                            ├── NBFC backend
                                            ├── safety policy, leases, verification
                                            └── systemd READY / WATCHDOG / STOPPING
```

- The **CLI** (`neosense`) and **TUI** (`neosense tui`) read state from the
  supervisor and never collect hardware data or invoke NBFC for live values.
- The **supervisor** (`neosensed`) is the only NBFC client and the only fan
  writer. It runs as a systemd service with a watchdog and an `ExecStopPost`
  fail-safe.
- The **shell installer** only handles installation, removal and systemd
  integration.

## Capabilities

| Area | What you get |
|---|---|
| Modes | **Auto** (NBFC curve), **Silent 25%**, **Balanced 60%**, **Maximum 100%** |
| Manual control | CPU, GPU or both logical fans, 20–100%, with confirmation |
| Profiles | Quiet / Balanced / Performance closed loops and persistent custom curves |
| Leases | 15-minute manual/preset lease with supervisor-verified recovery |
| Interfaces | Ratatui TUI, human CLI, stable `--json` output, shell completions |
| Diagnostics | Read-only `diagnose`, `control readiness`, `hardware identity`, validation history |
| Service | systemd `Type=notify`, watchdog, sandboxing, root-owned IPC socket |

## Install

### Requirements

- Linux with systemd and hwmon/thermal sensors. `x86_64` is the tested target.
- A configured NBFC installation, **0.5.4 or newer** for protected manual control
  (0.5.3 supports read-only diagnostics and `auto`).
- `sudo` for installation. Rust/Cargo 1.85+ is needed only for source builds.
- For the socket, your user must join the `neosense` group after installation.

### From the release archive

Download both assets from the
[latest release](https://github.com/NeonMC23/NeoSense/releases/latest), verify the
checksum, extract and install:

```bash
VERSION=2.4.0
curl -LO "https://github.com/NeonMC23/NeoSense/releases/download/v${VERSION}/neosense-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
curl -LO "https://github.com/NeonMC23/NeoSense/releases/download/v${VERSION}/SHA256SUMS"
sha256sum -c SHA256SUMS
tar -xzf "neosense-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
cd "neosense-${VERSION}-x86_64-unknown-linux-gnu"
sudo ./neosense.sh
```

Run the installer only from a trusted checkout or a checksum-verified archive: it
executes bundled shell modules and package-manager operations as root.

### From source

```bash
git clone https://github.com/NeonMC23/NeoSense.git
cd NeoSense
sudo ./neosense.sh
```

`neosense.sh` detects the host, checks dependencies, prepares a supported backend
when possible, installs the software and runs a read-only verification. Useful
options:

```text
--dry-run          show privileged actions without performing them
--yes              authorize package and software installation non-interactively
--software-only    install without a ready fan backend; does not start control
--no-start         do not enable or start the supervisor
--no-restart       leave an already-running supervisor untouched
--install-backend  allow a distro NBFC package install (explicit consent)
```

A missing or incompatible backend stops the default install **before** NeoSense
software changes. The installer never downloads arbitrary upstream scripts and
never modifies NBFC configuration. See [`docs/INSTALL.md`](docs/INSTALL.md) for
the full dependency matrix, distribution status and recovery rules.

## First run

```bash
# Join the service group, then start a new login session.
sudo usermod -aG neosense "$USER"
# log out and back in, or: newgrp neosense

# If the installer did not start the service (e.g. --no-start):
sudo systemctl enable --now neosensed

neosense service status
neosense status
neosense tui
```

`neosense service status` distinguishes healthy, degraded, permission-denied,
not-running and stale-socket states. Without the group, the CLI can read local
files but cannot reach the supervisor socket.

## Everyday use

### Presets and automatic mode

```bash
neosense auto                # return all fans to NBFC automatic mode
neosense silent --yes        # fixed 25% on both logical fans
neosense balanced --yes      # fixed 60%
neosense maximum --yes       # fixed 100%
```

Silent, Balanced and Maximum are fixed logical targets, not hidden temperature
curves. They use the same policy, backend verification and 15-minute lease as
manual control. A fixed target is a backend percentage, not physical RPM.

### Manual control

```bash
neosense set cpu 50 --yes    # one logical channel
neosense set gpu 50 --yes
neosense set both 50 --yes   # both channels, all-or-nothing preflight
neosense auto fan 0          # return one fan to automatic
neosense auto                # return all fans to automatic
```

Manual targets are bounded to **20–100%**; 0% is refused. The supervisor
preflights every requested fan and refuses the operation when the safety policy
cannot establish a safe state. A mid-operation backend failure is reported as a
structured partial failure and triggers automatic recovery.

### Profiles

Profiles are temperature-driven curves, distinct from fixed presets.

```bash
neosense profile list                 # catalogue and compatibility
neosense profile preview quiet        # local calculation, no write
neosense profile apply quiet --yes    # supervisor-owned closed loop
neosense profile state
neosense profile clear                # end regulation, return to automatic
```

Eligible fans are regulated; blocked fans return to automatic and are listed
with reasons. Built-in curves are Quiet, Balanced and Performance. Custom curves
are stored in `/var/lib/neosense/profiles.json` and edited with
`neosense profile custom ...` or from the TUI Profiles view. See
[`docs/PROFILES.md`](docs/PROFILES.md) and
[`docs/CUSTOM_PROFILES.md`](docs/CUSTOM_PROFILES.md).

### JSON and automation

Every command that reports state supports `--json`, including `status`,
`diagnose`, `fans`, `sensors`, `hardware identity`, `control readiness` and
profile evaluations. JSON errors use a stable `{"error":{"code","message","details"}}`
shape with the documented process exit code.

```bash
neosense status --json | jq '.control.stage'
neosense diagnose --json | jq '.checks[] | select(.status != "ok")'
```

## Terminal UI

`neosense tui` is the primary interactive interface, designed for an 80×24
terminal and usable smaller.

| View | Purpose |
|---|---|
| **Dashboard** | Mode, both fans, temperatures, safety and one-key mode switching |
| **Fan Control** | Select CPU, GPU or **Both**, set or adjust a target, return to automatic |
| **Presets** | Silent / Balanced / Maximum cards or list, applied immediately |
| **Profiles** | Apply, preview, deactivate and edit thermal profiles |
| **Sensors** | Role, value, trust, status, provider and path |
| **System** | NBFC, supervisor, IPC socket and active warnings |
| **Diagnostics** | Control readiness, calibration prerequisites and grouped checks |

The masthead adapts to the terminal: full banner with telemetry at large sizes,
a compact two-row wordmark at medium sizes and a one-line wordmark on small
terminals. A strict printable-ASCII banner is used on non-UTF-8 locales or with
`NEOSENSE_ASCII=1`.

| Key | Action |
|---|---|
| `Tab` / `←` `→` | Change view (`1`–`7` jump directly) |
| `s` / `b` / `m` / `a` | Dashboard: Silent / Balanced / Maximum / Auto |
| `↑` `↓` | Select a fan, preset or profile |
| `Enter` | Apply the selected preset or profile |
| `m` / `b` | Fan Control: set selected fan / both fans |
| `+` / `-` | Fan Control: adjust by 5% |
| `a` | Automatic mode: Dashboard, Fan Control selection or Profiles |
| `A` | Return all fans to automatic (any view) |
| `p` / `d` | Profiles: preview / deactivate |
| `r` / `?` / `q` | Refresh, help, quit |

Mode changes are immediate. Manual percentage changes still ask `y`/`n` because
they write fan targets directly. See [`docs/TUI.md`](docs/TUI.md) for the full
key map.

## Safety model

1. **Single writer.** Only `neosensed` writes fan state. The CLI and TUI never
   open NBFC, EC, ACPI, WMI or PWM interfaces directly.
2. **No invented values.** Missing, invalid, stale or unreadable sensors are
   `null`/`unavailable`, never `0 °C`.
3. **Evidence before control.** Manual control requires fresh trusted
   temperatures, a known mapping, backend guarantees and explicit confirmation.
4. **Bounded targets.** Manual percentages are 20–100%; every target is a lease.
5. **No false success.** Requested value, backend target, EC readback and
   physical response remain separate.
6. **Fail safe.** Lease expiry, lost sensors, NBFC critical mode, failed
   verification or supervisor failure trigger automatic recovery. A failed
   recovery is recorded as `fail-safe`, retried twice with a short backoff and
   then reported through supervisor health so systemd can restart the service;
   startup and `ExecStopPost` re-assert automatic mode.
7. **Per-fan profile gating.** Profiles apply only to fans that pass policy;
   the rest stay automatic.
8. **Actionable warnings.** Every warning carries a severity, cause and next safe
   step; colour is never the only signal.

The IPC socket is `root:neosense`, mode `0660`; the supervisor applies these
permissions before signalling readiness. See [`docs/SYSTEMD.md`](docs/SYSTEMD.md).

## Diagnostics

Start read-only:

```bash
neosense status
neosense diagnose
neosense control readiness --json
neosense service status
neosense sensors list
neosense hardware identity
neosense validation history
```

| Symptom | Meaning | Next step |
|---|---|---|
| `Permission denied` on the socket | User is not in `neosense`, or the session predates group membership | `sudo usermod -aG neosense "$USER"`, then log in again |
| `Supervisor unavailable` | `neosensed` is not running or is unhealthy | `systemctl status neosensed`; `sudo systemctl enable --now neosensed` |
| Manual control refused (exit 8) | Safety policy: temperature, mapping, insecure NBFC socket or NBFC < 0.5.4 | Read the reason; `neosense diagnose` lists blockers |
| EC readback shows 100% at a lower target | NBFC clamps raw EC words against its write range | Treat EC as a backend percentage, not RPM |
| GPU control blocked | NBFC declares `@CPU`; no fresh NVIDIA die temperature is available | Keep automatic control; check the NVIDIA driver |
| `stale` / `not fresh` | The supervisor snapshot aged out | `neosense sensors watch`; check supervisor health |
| Profile partially applied | Some fans failed policy | Blocked fans and reasons are reported; they stay automatic |

Exit codes: `0` success · `2` usage · `3` not found · `4` permission ·
`5` diagnostic failure · `6` degraded · `7` backend/service unavailable ·
`8` unsafe operation refused · `9` configuration/profile invalid ·
`10` verification failed.

## Supported platforms

| Item | Status |
|---|---|
| Acer Nitro AN515-57, i5-11400H, RTX 3050, patched NBFC 0.5.4 | Primary validated machine; CPU/GPU actuator response operator-attested, no RPM calibration |
| NBFC 0.5.4+ | Recommended; protected manual control |
| NBFC 0.5.3 | Read-only diagnostics and `auto`; manual control refused |
| Fedora / Nobara, Debian / Ubuntu, Arch, openSUSE | Detection and package adapters implemented; full flow mock-tested, hardware validation only on the Acer target |
| Other laptops | Validate read-only with `neosense diagnose --json` before enabling control |
| `x86_64` | Tested release target |
| `aarch64` source builds | Accepted by the installer; no hardware validation |

## Known limitations

- **Physical validation is operator-attested, not RPM calibration.** The CPU
  channel was accepted at 0/50/100%; the GPU channel was observed at
  50/75/100% with CPU automatic and verified Auto recovery. A percentage target
  does not prove the same physical speed.
- **No independent RPM/airflow reference.** The kernel exposes no Acer hwmon fan
  input; provisional EC tach words are raw candidates and never enter
  regulation.
- **EC readback is not RPM.** `IndependentReadMinMaxValues=false` makes
  `CurrentSpeed=100%` a clamp artifact in this configuration.
- **GPU telemetry is indirect.** Both NBFC channels declare `@CPU`; the GPU
  channel may use a fresh trusted NVIDIA die temperature as a guard, not as
  physical fan mapping.
- **Profiles are reference curves**, not measured fan-response curves.
- **Single-machine evidence.** No broad laptop compatibility claim is made.
- **`fan release` is intentionally unavailable** until `reset-ec`/firmware
  return is validated.
- **VM validation is software-only.** The Fedora 44 VM exercised install,
  systemd, upgrade, rollback and uninstall without EC/fan hardware; it is not
  backend-ready hardware validation.

Hardware evidence and boundaries:
[`docs/HARDWARE.md`](docs/HARDWARE.md) ·
[`docs/VALIDATION_RESULTS.md`](docs/VALIDATION_RESULTS.md) ·
[`docs/CALIBRATION_READINESS.md`](docs/CALIBRATION_READINESS.md) ·
[`docs/PHYSICAL_VALIDATION_PROTOCOL.md`](docs/PHYSICAL_VALIDATION_PROTOCOL.md).

## Documentation

| Document | Contents |
|---|---|
| [`docs/CLI.md`](docs/CLI.md) | CLI reference, JSON schemas, exit codes |
| [`docs/TUI.md`](docs/TUI.md) | Views, keys, interaction and safety behavior |
| [`docs/INSTALL.md`](docs/INSTALL.md) | Installer, dependency matrix, distribution status, recovery |
| [`docs/HOST_INSTALL.md`](docs/HOST_INSTALL.md) | Root deployment history, upgrade, rollback, uninstall |
| [`docs/SYSTEMD.md`](docs/SYSTEMD.md) | Unit, watchdog, IPC and fail-safe behavior |
| [`docs/PROFILES.md`](docs/PROFILES.md) | Thermal curve engine and application boundaries |
| [`docs/CUSTOM_PROFILES.md`](docs/CUSTOM_PROFILES.md) | Persistent curve schema, editing and recovery |
| [`docs/HARDWARE.md`](docs/HARDWARE.md) | Per-fan evidence, registers and readback semantics |
| [`docs/VALIDATION_RESULTS.md`](docs/VALIDATION_RESULTS.md) | Physical validation record |
| [`docs/CALIBRATION_READINESS.md`](docs/CALIBRATION_READINESS.md) | Calibration prerequisites and blockers |
| [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md) | Implementation status and resolved findings |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Feature status, evidence and next steps |
| [`docs/RELEASE_NOTES_2.4.0.md`](docs/RELEASE_NOTES_2.4.0.md) | Current release notes and limitations |
| [`CHANGELOG.md`](CHANGELOG.md) | User-visible changes |
| [`docs/RELEASE.md`](docs/RELEASE.md) | Release process and checklist |

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --locked
./packaging/test-installer-isolated.sh
./installer/tests/run-tests.sh
./packaging/test-release-archive.sh dist/neosense-*.tar.gz
```

Tests never write fan state. They cover NBFC parsing, sensor fixtures, NVIDIA
parsing, policy, presets, profiles, storage recovery, IPC, daemon lifecycle, TUI
rendering and installer rollback. Hardware-dependent checks are separate and
require explicit authorization. See [`CONTRIBUTING.md`](CONTRIBUTING.md) and
[`SECURITY.md`](SECURITY.md).

## License

MIT © 2026 NeoSense Open Source Contributors. See [`LICENSE`](LICENSE).
