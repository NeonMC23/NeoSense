# NeoSense

**Fan control for Linux laptops — one safe supervisor, a clear terminal UI and a scriptable CLI.**

NeoSense is a Rust application that puts a practical interface on top of
[NBFC (Notebook FanControl)](https://github.com/hirschmann/nbfc). It gives you
four operating modes, per-fan manual targets and thermal profiles, while keeping
every fan write behind a single supervised backend with leases, verification and
automatic recovery.

> NeoSense does not replace NBFC. NBFC still owns the embedded controller, the
> hardware configuration and the vendor critical-temperature handling. NeoSense
> owns the user workflow and the safety policy in front of it.

```text
███╗   ██╗ ███████╗  ██████╗  ███████╗ ███████╗ ███╗   ██╗ ███████╗ ███████╗
████╗  ██║ ██╔════╝ ██╔═══██╗ ██╔════╝ ██╔════╝ ████╗  ██║ ██╔════╝ ██╔════╝
██╔██╗ ██║ █████╗   ██║   ██║ ███████╗ █████╗   ██╔██╗ ██║ ███████╗ █████╗
██║╚██╗██║ ██╔══╝   ██║   ██║ ╚════██║ ██╔══╝   ██║╚██╗██║ ╚════██║ ██╔══╝
██║ ╚████║ ███████╗ ╚██████╔╝ ███████║ ███████╗ ██║ ╚████║ ███████║ ███████╗
╚═╝  ╚═══╝ ╚══════╝  ╚═════╝  ╚══════╝ ╚══════╝ ╚═╝  ╚═══╝ ╚══════╝ ╚══════╝
```

A bold ANSI Shadow-inspired wordmark, presented as a masthead with the
`LINUX THERMAL MANAGEMENT` descriptor, version, supervisor state, NBFC version
and machine model. It is rendered from Unicode block characters on UTF-8
terminals; on non-UTF-8 locales, or with `NEOSENSE_ASCII=1`, the banner switches
to a strict printable-ASCII six-row fallback.

**Project status:** 2.4.0 release candidate, not publicly released; 2.3.0
remains a frozen unpublished artifact. Linux only — there are no Windows or macOS builds.

---

## Why NeoSense

- **Everyday control that fits in two keys.** `neosense silent`, `balanced`,
  `maximum` or `auto` — or open the TUI and pick a mode from the Dashboard.
- **Manual control that is honest.** CPU, GPU or both fans at 20–100%, with the
  requested value, the backend-accepted target and the EC readback kept
  separate. EC percentages are never presented as RPM.
- **One writer.** Only the `neosensed` supervisor talks to NBFC and hardware.
  The CLI and TUI speak a local JSON protocol and stay read-only if the
  supervisor is unavailable.
- **Fail-safe by design.** Fixed targets are leases. Expiry, stale telemetry, a
  critical backend state, a failed verification or a supervisor crash returns
  fans to NBFC automatic mode.
- **Bounded closed-loop profiles.** Quiet, Balanced and Performance curves are
  applied per fan only when policy allows; blocked fans stay automatic and are
  reported.
- **No invented data.** Missing, stale or untrusted sensors are `null` or
  `unavailable`; they are never shown as `0 °C` or as a successful fan write.
- **One install entry point.** `neosense.sh` detects the distribution, checks
  versioned dependencies, reports backend/controller conflicts and verifies the
  result. It never changes NBFC configuration or stops another controller.

## At a glance

| | |
|---|---|
| Interfaces | `neosense` CLI, Ratatui TUI (`neosense tui`) |
| Fan backend | NBFC (0.5.4 or newer recommended) |
| Modes | **Auto**, **Silent 25%**, **Balanced 60%**, **Maximum 100%** |
| Manual control | CPU, GPU or both fans, 20–100% |
| Profiles | Closed-loop Quiet/Balanced/Performance plus persistent custom curves |
| Lease | 15 minutes, then supervisor-verified automatic recovery |
| Platform | Linux with systemd and hwmon/thermal sensors |
| Tested on | Acer Nitro AN515-57, Nobara 44, kernel 7.3, patched NBFC 0.5.4 |

---

## Quick start

**Requirements:** Linux, systemd, a configured NBFC installation, and `sudo`
for installation. Rust/Cargo is needed only for a source install.

### Install

From a release archive:

```bash
sha256sum -c SHA256SUMS
tar -xzf neosense-2.4.0-x86_64-unknown-linux-gnu.tar.gz
cd neosense-2.4.0-x86_64-unknown-linux-gnu
sudo ./neosense.sh
```

From a source checkout:

```bash
git clone git@github.com:NeonMC23/NeoSense.git
cd NeoSense
sudo ./neosense.sh
```

Only run the installer from a trusted checkout or a checksum-verified archive:
it executes the bundled shell modules and package-manager operations as root.

`neosense.sh` detects the distribution, checks and prepares dependencies, then
installs and verifies NeoSense. See [`docs/INSTALL.md`](docs/INSTALL.md) for the
dependency matrix, distribution status and recovery rules. Use `--dry-run` to inspect the plan, `--yes` for
non-interactive authorization, and `--software-only` when no fan backend is
available. The low-level `install.sh` remains available for packaging and
recovery workflows; the unified installer delegates to it.

Allow your user to reach the supervisor socket, then start the service:

```bash
sudo usermod -aG neosense "$USER"
# log out and back in, or run: newgrp neosense
sudo systemctl enable --now neosensed
```

### First commands

```bash
neosense status              # thermal and fan summary
neosense auto                # NBFC automatic mode
neosense silent --yes        # fixed 25% on both logical fans
neosense balanced --yes      # fixed 60%
neosense maximum --yes       # fixed 100%
neosense set both 50 --yes   # manual 50%
neosense auto                # back to automatic
```

Open the terminal UI with:

```bash
neosense tui
```

---

## Everyday use

### Operating modes

| Mode | What it does |
|---|---|
| **Auto** | Returns both fans to NBFC's temperature-dependent automatic curve. This is the normal automatic behavior on this laptop. |
| **Silent** | Fixed 25% target on both logical fan channels. |
| **Balanced** | Fixed 60% target on both logical fan channels. |
| **Maximum** | Fixed 100% target on both logical fan channels. |

Silent, Balanced and Maximum are **fixed logical targets**, not hidden
temperature curves. They use the same supervisor policy, backend verification
and 15-minute lease as manual control. When the lease expires, the supervisor
returns the fans to NBFC automatic mode. A fixed target does not prove a
physical speed: percentages are backend values, and EC readback is not RPM.

### Manual control

```bash
neosense set cpu 50 --yes     # CPU logical channel
neosense set gpu 50 --yes     # GPU logical channel
neosense set both 50 --yes    # both channels
neosense auto fan 0           # return one fan to automatic
neosense auto                 # return all fans to automatic
```

Manual targets are bounded to **20–100%**; 0% is deliberately refused. The
supervisor preflights every requested fan and refuses the operation if the
safety policy cannot establish a safe state. `set both` is all-or-nothing at
preflight; a mid-operation backend failure is reported as a structured partial
failure and triggers automatic recovery.

### Thermal profiles

Profiles are temperature-driven curves, distinct from fixed presets.

```bash
neosense profile list                 # catalogue and current compatibility
neosense profile preview quiet        # local calculation, no write
neosense profile apply quiet --yes    # supervisor-owned closed loop
neosense profile clear                # end regulation, return to automatic
```

Applying a profile is policy-gated per fan. Eligible fans are regulated; blocked
fans are returned to automatic and listed with reasons. A profile can be
partial. Built-in curves are Quiet, Balanced and Performance; custom curves are
stored in `/var/lib/neosense/profiles.json` and edited with
`neosense profile custom ...` or from the TUI's Profiles view.

### Terminal UI

`neosense tui` is the primary interactive interface. It is designed for a
normal 80×24 terminal and stays usable at smaller sizes.

| View | Purpose |
|---|---|
| **Dashboard** | Current mode, both fan states, temperatures, safety and one-key mode switching |
| **Fan Control** | Select CPU, GPU or **Both**, set a target, adjust by 5%, return to automatic |
| **Presets** | Browse Silent / Balanced / Maximum and apply immediately |
| **Profiles** | Apply or preview thermal profiles, deactivate, edit custom curves |
| **Sensors** | Role, value, trust, status, provider and path |
| **System** | NBFC, supervisor, IPC socket and active warnings |
| **Diagnostics** | Control readiness, calibration prerequisites and grouped checks |

The header is responsive. Large terminals (90×30 or more) show the full banner
with a telemetry rail or status line; medium terminals (70×20 or more) show a
compact two-row wordmark with version, supervisor, NBFC and host; small
terminals use a one-line wordmark so the dashboard keeps its working area.

Common keys:

| Key | Action |
|---|---|
| `Tab` / `←` `→` | Change view (`1`–`7` jump directly) |
| `s` / `b` / `m` / `a` | Dashboard: Silent / Balanced / Maximum / Auto |
| `↑` `↓` | Select a fan, preset or profile |
| `Enter` | Apply the selected preset or profile immediately |
| `m` / `b` | Fan Control: set the selected fan / both fans |
| `+` / `-` | Fan Control: adjust by 5% |
| `a` | Contextual: Dashboard Auto, Fan Control selected fan automatic, Profiles apply |
| `A` | Return all fans to automatic and end an active profile |
| `p` | Profiles: preview without writing |
| `d` | Profiles: deactivate the active profile |
| `r`, `?`, `q` | Refresh, help, quit |

Mode selection is immediate: choosing Auto, a preset or a profile needs no extra
confirmation. Manual percentage changes still ask `y`/`n` because they change
fan targets directly. See [`docs/TUI.md`](docs/TUI.md) for the full key
map and interaction rules.

---

## Installation details

### What the installer writes

- the unified entry point and helper modules stay in the release archive or
  source checkout (they are not copied into the system prefix)
- `/usr/local/bin/neosense` and `/usr/local/bin/neosensed`
- `/etc/systemd/system/neosensed.service`
- `/run/neosense/` (`root:neosense`, mode `0750`)
- the `neosense` system group, if it does not already exist

The service owns `/var/lib/neosense` through systemd `StateDirectory`; existing
profiles and validation evidence are preserved across upgrades.

The installer does **not** start an inactive supervisor. If the supervisor is
already running, it restarts and verifies it after a successful upgrade;
`--no-restart` leaves the process running and prints the explicit restart step.
A running supervisor's designed shutdown fail-safe may return NBFC to automatic
mode, so this is not a zero-write upgrade.

Before replacing files, the installer creates a timestamped backup under
`/var/lib/neosense/backups/` and restores the previous binaries and unit if
installation or the post-install check fails.

### Upgrade

Run `sudo ./neosense.sh upgrade` from the new checkout or archive. Existing
profiles, state and validation records are retained; the installer verifies the
result and reports any backend or hardware limitation. See
[`docs/INSTALL.md`](docs/INSTALL.md) for the dependency matrix, distribution
status and recovery rules, and [`docs/HOST_INSTALL.md`](docs/HOST_INSTALL.md)
for the root deployment history.

### Uninstall

```bash
sudo ./neosense.sh uninstall
```

`uninstall.sh` remains the low-level uninstaller used by that command.

This stops the supervisor (triggering automatic recovery) and removes
NeoSense-owned binaries, the unit and the runtime directory. If a profile store
or validation evidence exists under `/var/lib/neosense`, that state directory
(including its backups) is preserved for manual review; otherwise it is removed.
The `neosense` group is removed only when it is empty and has no primary users.
NBFC configuration is untouched.

### Release archive

```bash
./packaging/build-release.sh
./packaging/test-release-archive.sh dist/neosense-*.tar.gz
```

The archive contains prebuilt binaries, `neosense.sh`, the low-level
installer/uninstaller, the systemd unit, documentation, completions, build
information and checksums.

---

## Safety model

1. **Single writer.** Only `neosensed` writes fan state. The CLI and TUI never
   open the NBFC socket, EC, ACPI, WMI or PWM interfaces.
2. **No invented values.** Missing, invalid, stale or unreadable sensors are
   `null`/`unavailable`, never `0 °C`.
3. **Evidence before control.** Manual control requires valid, fresh, trusted
   temperature data, a known fan mapping and explicit confirmation.
4. **Bounded targets.** Manual percentages are limited to 20–100%; each target
   is a lease with a 15-minute timeout.
5. **No false success.** Backend acceptance is not physical proof. Requested
   value, backend target, EC readback and physical response are separate.
6. **Fail safe.** Lease expiry, lost sensors, NBFC critical mode, failed
   verification or supervisor failure trigger automatic recovery; an
   unconfirmed recovery is reported as a distinct failure.
7. **Per-fan profile gating.** A closed-loop profile is applied only to fans
   that pass mapping, temperature and backend policy. Blocked fans stay
   automatic.
8. **Explicit warnings.** Every warning carries a severity, a cause and the next
   safe action. Colour is never the only signal.

The supervisor is the sole authority. `Type=notify` with `WatchdogSec=10s`
ensures a hung supervisor is terminated, and its `ExecStopPost` fail-safe
re-asserts NBFC automatic mode. See [`docs/SYSTEMD.md`](docs/SYSTEMD.md).

---

## Diagnostics and troubleshooting

Start read-only:

```bash
neosense status
neosense diagnose
neosense control readiness --json
neosense service status
neosense validation history
```

| Symptom | What it means | Next step |
|---|---|---|
| `Permission denied` on the socket | Your user is not in the `neosense` group, or the session predates group membership | `sudo usermod -aG neosense "$USER"`, then log in again |
| `Supervisor unavailable` | `neosensed` is not running or is unhealthy | `systemctl status neosensed`; `sudo systemctl enable --now neosensed` |
| Manual control refused (exit 8) | Policy gates: unsafe temperature, missing GPU telemetry, insecure NBFC socket, or NBFC older than 0.5.4 | Read the reported reason; `neosense diagnose` shows all blockers |
| `EC readback 100%` at a lower target | NBFC compares raw EC words against the write range and clamps them | Treat EC as a backend percentage, not RPM; see [`docs/HARDWARE.md`](docs/HARDWARE.md) |
| GPU control blocked | NBFC declares `@CPU` for the GPU channel and no fresh NVIDIA temperature is available | Keep automatic control; check the NVIDIA driver. Physical identity remains unverified |
| `not fresh` / `stale` | The supervisor snapshot aged out | `neosense sensors watch` and check `neosensed` health |
| Profile partially applied | Some fans failed policy; the rest are regulated | Blocked fan indexes and reasons are reported; blocked fans stay automatic |

Exit codes: `0` success, `2` usage, `3` not found, `4` permission, `5`
diagnostic failure, `6` degraded, `7` backend/service unavailable, `8` unsafe
operation refused, `9` configuration/profile invalid, `10` verification failed.

---

## Known limitations

- **Physical validation is operator-attested, not RPM calibration.** The CPU
  logical channel was accepted at 0/50/100%; the GPU logical channel was
  observed at 50/75/100% with CPU automatic and verified Auto recovery. A 50%
  target does not prove 50% physical speed. See
  [`docs/VALIDATION_RESULTS.md`](docs/VALIDATION_RESULTS.md).
- **No independent RPM reference.** The kernel exposes no Acer hwmon fan input.
  Provisional EC tach words are shown as raw candidates and never enter
  regulation.
- **EC readback is not RPM.** `IndependentReadMinMaxValues=false` makes NBFC
  compare raw EC words against the 0..100 write range, so `CurrentSpeed=100%` is
  a clamp artifact.
- **GPU telemetry is indirect.** Both NBFC channels declare `@CPU`; the GPU
  logical channel may use a fresh, trusted NVIDIA die temperature as a guard.
  This is reported as a warning and does not establish physical fan identity.
- **NBFC automatic is the normal automatic behavior.** `AutoMode=true` is
  NBFC's own temperature-dependent software curve. The config's manual EC mode
  bytes remain NBFC-managed, which is expected.
- **Profiles are reference curves.** They are not derived from a measured
  fan-response curve on this laptop.
- **Single-machine evidence.** No broad laptop compatibility claim is made.
  Validate read-only with `neosense diagnose --json` before enabling control.
- **`fan release` is intentionally unavailable** until `reset-ec`/firmware
  return is validated.

Hardware evidence and backend boundaries are documented in
[`docs/HARDWARE.md`](docs/HARDWARE.md),
[`docs/CALIBRATION_READINESS.md`](docs/CALIBRATION_READINESS.md) and
[`docs/PHYSICAL_VALIDATION_PROTOCOL.md`](docs/PHYSICAL_VALIDATION_PROTOCOL.md).

---

## Architecture

```text
 neosense (CLI)  ──┐
 neosense tui     ─┴── Unix JSON IPC ──► neosensed (root, sole writer)
                                            ├── sensor registry (hwmon/thermal/NVIDIA)
                                            ├── NBFC backend
                                            ├── safety policy + leases + verification
                                            └── systemd READY / WATCHDOG / STOPPING
```

- The CLI and TUI read backend state and the sensor snapshot from the supervisor
  over IPC. They never collect sensors or invoke NBFC for live state.
- All fan, preset and profile writes use the supervisor IPC protocol.
- The root supervisor is the only NBFC client.
- The shell is limited to installation, removal and systemd integration.

---

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo build --release --locked
./packaging/test-installer-isolated.sh
./installer/tests/run-tests.sh
git diff --check
```

Tests never write fan state. They cover NBFC parsing, sensor fixtures, NVIDIA
parsing, policy, presets, profiles, storage recovery, IPC, daemon lifecycle, TUI
rendering and installer rollback. Hardware-dependent checks are separate; use
`neosense diagnose --json` and explicit authorization for anything that touches
real fans.

See [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`SECURITY.md`](SECURITY.md).

---

## Documentation

| Document | Contents |
|---|---|
| [`docs/CLI.md`](docs/CLI.md) | Complete CLI reference, JSON schemas, exit codes |
| [`docs/TUI.md`](docs/TUI.md) | Views, keys, interaction rules and safety behavior |
| [`docs/PROFILES.md`](docs/PROFILES.md) | Thermal curve engine and application boundaries |
| [`docs/CUSTOM_PROFILES.md`](docs/CUSTOM_PROFILES.md) | Persistent curve schema, editing and recovery |
| [`docs/HARDWARE.md`](docs/HARDWARE.md) | Per-fan evidence, registers and readback semantics |
| [`docs/VALIDATION_RESULTS.md`](docs/VALIDATION_RESULTS.md) | Authoritative CPU/GPU physical validation record |
| [`docs/CALIBRATION_READINESS.md`](docs/CALIBRATION_READINESS.md) | Calibration prerequisites and current blockers |
| [`docs/INSTALL.md`](docs/INSTALL.md) | Unified installer, dependency matrix, distribution status, recovery |
| [`docs/HOST_INSTALL.md`](docs/HOST_INSTALL.md) | Root installation history, upgrade, rollback, uninstall |
| [`docs/SYSTEMD.md`](docs/SYSTEMD.md) | Unit, watchdog and fail-safe behavior |
| [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md) | Implementation status and resolved findings |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Feature status, evidence and blockers |
| [`docs/CLI_AUDIT.md`](docs/CLI_AUDIT.md) | Command inventory and validation matrix |
| [`docs/HISTORICAL.md`](docs/HISTORICAL.md) | v1.0.1 feature comparison |
| [`docs/NBFC_ACCESS_CONTROL.md`](docs/NBFC_ACCESS_CONTROL.md) | NBFC socket access-control design |
| [`docs/PHYSICAL_VALIDATION_PROTOCOL.md`](docs/PHYSICAL_VALIDATION_PROTOCOL.md) | Authorized validation workflow and evidence format |
| [`docs/RELEASE.md`](docs/RELEASE.md) | Release checklist and artifact process |
| [`CHANGELOG.md`](CHANGELOG.md) | User-visible changes |

## Compatibility

| Item | Status |
|---|---|
| Acer Nitro AN515-57, i5-11400H, RTX 3050, NBFC 0.5.4 | Read-only verified; CPU/GPU actuator response operator-attested, no RPM calibration |
| NBFC 0.5.4+ | Recommended; protected manual control |
| NBFC 0.5.3 | Read-only diagnostics and `auto`; manual control refused |
| Other laptops | Validate individually with `neosense diagnose --json` |
| Fedora/Nobara, Debian/Ubuntu, Arch, openSUSE | Detection and package adapters implemented; full install flow mock-tested; hardware validation only on the Acer target |
| iGPU / no GPU sensor | No invented GPU temperature; GPU control unavailable |
| No RPM feedback | Verification limited; reported by `diagnose` |

## License

MIT. See [`LICENSE`](LICENSE).
