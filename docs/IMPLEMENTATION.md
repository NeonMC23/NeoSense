# NeoSense — Implementation status (v2.4.0 development)

**Date:** 2026-11-11 (production `2afe3ed` deployed; control-readiness cycle)
**Scope:** Rust migration, supervisor hardening, TUI, profile compatibility and local validation.

## Architecture

- `neosense`: Rust CLI. Backend state and the sensor snapshot come from the supervisor over IPC; only offline NBFC configuration files are read locally. Control uses IPC.
- `neosensed`: Rust supervisor and sole fan-write authority.
- Core modules: typed sensors, hwmon/thermal/NVIDIA providers, NBFC backend, safety policy, profiles, warnings, diagnostics, IPC.
- `neosense tui`: Ratatui/Crossterm UI sharing the same core and IPC protocol.
- `hardware identity`: read-only config plus live-sensor inspection that reports
  the effective read range, live neutral RPM channels, feedback eligibility and
  that physical CPU/GPU identity and control are unverified.
- Shell: installer, uninstaller and systemd integration only.

## Resolved findings

| Finding | Status |
|---|---|
| Case-sensitive status parser | Resolved: exact NBFC text parser + JSON 0.5.4, fixtures |
| Invalid `manual` command | Resolved: supervisor-mediated control; `set -s` argument tested |
| Lowercase boolean handling | Resolved |
| Generic GPU `temp1` fallback | Resolved: typed GPU providers; Wi-Fi/NVMe/ACPI keep their roles |
| EC percentage treated as truth | Mitigated: requested/target/EC/RPM separated, warnings and diagnostics |
| Fixed dangerous presets | Removed: only `safe-auto` is hardware-applicable; closed-loop profiles are simulation-only |
| Missing tests/CI | Resolved: 203 unit, 50 CLI, 6 installer tests and 1 language guard |
| Version drift | Resolved: `Cargo.toml` is the single version source |
| Missing warning system | Resolved: shared severity-labelled warnings in CLI/TUI |
| Unvalidated profile catalogue | Resolved: compatibility evaluation plus deterministic preview/simulation engine |
| No persistent user curves | Resolved: versioned store, strict validation, atomic writes, backup/recovery, CLI/TUI editor and IPC CRUD |
| NBFC socket bypass | **Resolved on the host**: patched 0.5.4 socket `0600 root:root`, root-only policy, supervisor-only client model, ordinary-user denial verified |
| CLI read backend directly | Resolved: CLI/TUI use supervisor IPC; an ordinary-user NBFC denial can no longer be misreported as a backend outage |
| CLI/TUI read hardware directly | Resolved: sensor snapshots now come from the supervisor. `sensors list/watch`, status, diagnostics, fan status and hardware identity no longer collect `/sys` or run `nvidia-smi` in the client process. `hardware list` reads configuration files and `hardware validate` is a supervisor IPC method |
| Unbounded IPC connections | Resolved: concurrent handler threads are capped by `DaemonConfig::max_connections`; excess connections are closed without spawning a thread |
| Stale serialized freshness | Resolved: client reads recompute `health.fresh` from the supervisor sample timestamp, so an aged snapshot cannot claim freshness |
| Stale NBFC version cache | Resolved: revalidation on service restart/TTL and at write gates |
| Misleading readback label | Resolved: `hardware identity` reports the effective read range and its source; a write-range inference is described as a clamp source, not an invalid independent range |
| “Fan control” overstatement | Resolved: diagnostics separate manual-path eligibility, physical feedback and physical control validation |
| Profile refresh lost update | Resolved: profile evaluation uses the selection current at commit time |
| Installed-build drift | Resolved: current production build `2afe3ed` deployed and verified byte-for-byte; backup and rollback recorded |
| Manifest/lifecycle drift | Resolved: installer tests assert the unit's fail-safe ExecStopPost, absence of reset-ec, readiness/watchdog and sandbox properties |
| Feedback provenance | `hardware identity` reports `rpm_source`, neutral `rpm_channels`, `ec_readback` and `physical_identity`; uncalibrated EC words and unused ACPI/WMI paths are never presented as RPM |
| Acer RPM feedback | Active read-only `acer_wmi` candidate exposes `hwmon6` `name=acer` fan1/fan2 at `0444`; NeoSense reports neutral `Acer Fan 1/2`, physical measurement, feedback eligibility, no PWM/platform_profile; CPU/GPU identity and calibration unverified |
| Feedback/regulation separation | `SensorHealth` exposes `usable_for_regulation`, `usable_for_feedback` and `physical_measurement`; the deprecated `usable_for_control` equals regulation eligibility. RPM can never enter temperature regulation, and feedback availability cannot enable control |
| Control-readiness model | `src/control.rs` produces `neosense.control_readiness/v1`: stage, runtime mode, profile mode, per-fan eligibility/blockers, and explicit control vs validation requirements. Hardware validation requires physical identity, RPM calibration and actuator-response evidence; the Acer state remains unvalidated |
| acer_wmi compatibility | Upstream source and the running module audited; gaming WMI GUID present, AN515-57 missing from the DMI quirk list; no module parameter, reload or WMI call was made |

## Profiles

Profile states: `compatible`, `compatible with warnings`, `preview only`, `unsupported`, `blocked for safety`.

- `safe-auto` is evaluated against NBFC availability, service state, CPU trust, GPU mapping, RPM and EC readback.
- `quiet`, `balanced` and `performance` have deterministic reference curves for
  local preview and supervisor simulation; they are not applied to hardware.
- User-created custom curves are persistent dynamic profiles. They use the same
  regulation engine and are preview/simulation-only. The store is
  `/var/lib/neosense/profiles.json` (systemd `StateDirectory`), with
  `NEOSENSE_PROFILES_PATH` override and an unprivileged client fallback.
- The store provides versioned JSON, strict validation, an advisory write lock,
  atomic replace + fsync, a previous-generation backup, corruption recovery and
  explicit reset that preserves the corrupt input.
- An unavailable profile cannot be applied and explains why.
- No profile simulation path writes fan state; `profile apply` remains limited
  to safe-auto.

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

## Release and installation

- `neosense sensors watch --once` emits one snapshot; with `--json` watch emits NDJSON.
- `neosense completions bash|zsh|fish|elvish|powershell` generates shell completions.
- `packaging/build-release.sh` creates a versioned tarball, `BUILD-INFO`, completions and `SHA256SUMS`.
- `install.sh` stages/verifies both binaries, checks the unit exists, backs up previous binaries/unit under `/var/lib/neosense/backups`, rolls back on post-install verification or `daemon-reload` failure, and restores on INT/TERM.
- `uninstall.sh` removes generated state but preserves `profiles.json` (user data) and leaves NBFC configuration untouched.
- `packaging/test-installer-isolated.sh` validates fresh install, injected failure rollback, reinstall and uninstall inside a user namespace without touching the host.
- `packaging/test-release-archive.sh` verifies archive contents and installs the prebuilt archive in a temporary root.
- NBFC access-control classification (`NbfcSocketSecurity`) inspects mode,
  ownership, Unix-socket type, verified supervisor identity, local group
  membership, primary-GID users, POSIX ACLs and connectability. Manual, preset
  and closed-loop hardware operations require `access_controlled`; a
  group/world-writable socket is a security warning and a hard gate, while
  `fan auto` remains available for recovery. `0660 root:neosense` does not pass
  when the group contains ordinary IPC users. Protected operations re-inspect
  the socket immediately before use; lost access control fails a manual/preset
  lease toward automatic/fail-safe recovery.
- An isolated upstream NBFC 0.5.4 access-control patch is prepared for review
  outside this repository: configurable owner/group/mode, secure default
  `0600 root:root`, and `SO_PEERCRED` UID/GID allowlists enforced before
  `set-fan-speed`. The patched service builds and its isolated tests pass.
  See `docs/NBFC_ACCESS_CONTROL.md` for the exact contract and limits.
- `src/nbfc_config.rs` reads the selected NBFC configuration without modifying
  it. It exposes per-fan register pairs, sensor sources, readback validity and
  identity limits. The installed Acer configuration has distinct CPU `19/55`
  and GPU `21/58` register pairs, both wired to `@CPU`, and invalid readback
  ranges (`MinSpeedValueRead == MaxSpeedValueRead == 0`). This is logical
  identity evidence, not physical fan validation.
- `src/regulation.rs` is the pure profile policy engine: reference curves,
  interpolation, floor/ceiling clamping, hysteresis, rate limiting, sensor
  freshness, fan mapping and explicit unavailable states. `profile preview` is
  local/no-write; `profile select` is supervisor simulation/no-write. Custom
  curves are stored, validated and evaluated by the same path. The current
  hardware mapping still blocks application.
- Supervisor refresh caches the immutable NBFC version and sensor configuration
  for 30 seconds (forced immediately when the selected config changes), instead
  of spawning `nbfc --version` and `nbfc sensors show` every sampling interval.
  A 12-second fake-backend measurement reduced those calls from 8/8 to 1/1
  while `nbfc status` remained at the sampling interval. RSS observations were
  around 3.3 MB before and 2.3–3.4 MB in repeated runs after the change; the RSS
  difference is not established and is not a benchmark claim. The measurement
  is a development-host fixture, not real-hardware performance.
- The supervisor only changes a pre-existing socket parent directory when it created that directory; this prevents a custom `--socket /tmp/...` run from modifying `/tmp` permissions.
- IPC reads are bounded to `MAX_MESSAGE` before allocating/parsing a line.
- See `docs/RELEASE.md` and `docs/ROADMAP.md`.

## systemd

- `Type=notify`, `NotifyAccess=main`, `WatchdogSec=10s`.
- `READY=1` after socket bind, permissions and initial state snapshot.
- `WATCHDOG=1` only while health checks pass.
- `STOPPING=1` on orderly shutdown.
- `ExecStopPost=neosensed --fail-safe`, which sets NBFC automatic mode and verifies the result.
- User-systemd transient tests with a fake NBFC backend validated READY, watchdog, stop, restart after SIGKILL and ExecStopPost.
- The final 2.4.0 build was installed on the Acer on 2026-11-10 with a
  root-protected backup and the authorized service restart. The installed
  service reports READY, the watchdog timestamp advances, the socket is
  `660 root:neosense`, and the profile store is `0600 root:neosense` under a
  `0750 root:neosense` StateDirectory. Non-root CLI/TUI IPC was validated under
  the `neosense` group.
- The old supervisor's authorized `nbfc set -a` automatic-mode fail-safe ran
  during the two installation/validation restarts. No manual fan target was
  requested.
- The installed artifact was built from `8d4ed79`. Source `b4f0a4d` adds the
  read-only `hardware identity` report and warnings; the system binaries must be
  reinstalled from the newer artifact for that command to replace the installed
  copy. The installed supervisor API is unchanged.

## TUI

Seven views: Dashboard, Sensors, Fans, Profiles, Presets, System, Diagnostics. Profile selection uses Up/Down and Enter; applying `safe-auto` requires `y` confirmation and goes through IPC. The Profiles view includes persistent custom curves; `e` edits the selected curve and `n` creates a new template through `$EDITOR`, then saves via supervisor IPC. No manual speed is exposed. Without a supervisor, the TUI is read-only.

Rendering tests cover 40×20, 80×24, 100×32 and 160×48. A pty 100×32 smoke test and rendered inspection at 40×20, 80×24 and 100×32 were performed; no screenshot is committed.

## Local validation

Confirmed: Acer Nitro AN515-57, BIOS V1.17, i5-11400H, RTX 3050 Laptop, Nobara 44, kernel 7.3.1, patched NBFC 0.5.4.

- CPU: coretemp package/cores; ACPI and x86_pkg thermal zones.
- GPU: NVIDIA via `nvidia-smi`; memory temperature N/A correctly unavailable.
- NVMe and Wi-Fi classified separately; Wi-Fi never promoted to GPU.
- NBFC: two logical fans with distinct register pairs (CPU 19/55, GPU 21/58), both use `@CPU`; GPU fan is not driven by GPU temperature. Both have invalid readback ranges.
- EC reports 100% against varying targets; `MinSpeedValueRead == MaxSpeedValueRead == 0`, so EC CurrentSpeed is not a trustworthy percentage; no RPM; warnings visible in CLI/TUI/diagnostics.
- CLI status/sensors/watch/diagnose/service/hardware (including `hardware identity`)/profile/TUI tested read-only.
- `hardware validate` returns code 4 because `rate-config` needs root.
- `service status` returns code 4 for a user outside the group.

## Test commands

```text
cargo fmt --all -- --check                  OK
cargo clippy --all-targets --all-features -- -D warnings  OK
cargo test --all-targets --all-features    OK (148 unit, 42 CLI, 4 installer, 1 language)
cargo build --release --locked             OK
shellcheck install.sh uninstall.sh         OK
bash -n install.sh uninstall.sh            OK
systemd-analyze verify (neutralized unit)  OK
```

No test writes fan state. The supervisor/TUI tests use fake backends or temporary sockets.

## Remaining work

- Real-host rollback exercise (the backup exists, but rollback was not executed because installation succeeded).
- Real NBFC ≥ 0.5.4 manual-control test with RPM and explicit user authorization.
- Hardware application of closed-loop/custom profiles, independent feedback and hardware matrix.
- Real physical fan identity/airflow mapping and RPM feedback validation.
- `fan release`/`reset-ec` validation.
- Human visual review of the TUI.
- NBFC socket `0666` remains an upstream limitation.
