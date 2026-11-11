# NeoSense — Implementation status (v2.4.0 development)

**Date:** 2026-11-10
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
| Fixed dangerous presets | Removed: only `safe-auto` is hardware-applicable; closed-loop profiles are simulation-only |
| Missing tests/CI | Resolved: 137 unit, 42 CLI, 4 installer tests and 1 language guard |
| Version drift | Resolved: `Cargo.toml` is the single version source |
| Missing warning system | Resolved: shared severity-labelled warnings in CLI/TUI |
| Unvalidated profile catalogue | Resolved: compatibility evaluation plus deterministic preview/simulation engine |
| No persistent user curves | Resolved: versioned store, strict validation, atomic writes, backup/recovery, CLI/TUI editor and IPC CRUD |
| NBFC socket bypass | Detected/reported and gated; upstream 0.5.3/0.5.4/master still hardcode `0666`, so exclusive access is not claimed |

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
  ownership and Unix-socket type. Manual, preset and closed-loop hardware
  operations now require `access_controlled`; a group/world-writable NBFC socket
  is a security warning and a hard gate, while `fan auto` remains available for
  recovery.
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

Confirmed: Acer Nitro AN515-57, BIOS V1.17, i5-11400H, RTX 3050 Laptop, Nobara 44, kernel 7.3.1, NBFC 0.5.3.

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
cargo test --all-targets --all-features    OK (137 unit, 42 CLI, 4 installer, 1 language)
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
