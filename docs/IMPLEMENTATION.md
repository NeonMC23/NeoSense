# NeoSense — Implementation status (v2.4.0 stable)

**Date:** 2026-11-19 (release audit; v2.4.0 published and installed on the development host)
**Scope:** Rust migration, supervisor hardening, TUI, profile compatibility and local validation.

## Architecture

- `neosense`: Rust CLI. Backend state and the sensor snapshot come from the supervisor over IPC; only offline NBFC configuration files are read locally. Control uses IPC.
- `neosensed`: Rust supervisor and sole fan-write authority.
- Core modules: typed sensors, hwmon/thermal/NVIDIA providers, NBFC backend, safety policy, profiles, warnings, diagnostics, IPC.
- `neosense tui`: Ratatui/Crossterm UI sharing the same core and IPC protocol.
- `hardware identity`: read-only config plus live-sensor inspection that reports
  the effective read range, live neutral RPM channels, feedback eligibility and
  that physical CPU/GPU channel identity and independent control measurement
  remain unverified; operator-attested actuator response is recorded separately.
- Shell: installer, uninstaller and systemd integration only.
- `neosense.sh`: unified Linux installation entry point. It performs host and
  distribution detection, versioned dependency checks, DNF/APT/Pacman/Zypper
  package preparation after consent, NBFC/backend state and conflict detection,
  delegates transactional file installation/rollback to `install.sh`, and
  finishes with read-only capability verification. Installer modules live under
  `installer/lib/`; isolated flow tests live under `installer/tests/`.

## Resolved findings

| Finding | Status |
|---|---|
| Case-sensitive status parser | Resolved: exact NBFC text parser + JSON 0.5.4, fixtures |
| Invalid `manual` command | Resolved: supervisor-mediated control; `set -s` argument tested |
| Lowercase boolean handling | Resolved |
| Generic GPU `temp1` fallback | Resolved: typed GPU providers; Wi-Fi/NVMe/ACPI keep their roles |
| EC percentage treated as truth | Mitigated: requested/target/EC/RPM separated, warnings and diagnostics |
| Fixed dangerous presets | Removed as unconditional defaults; presets and profiles are supervisor/policy-gated per fan |
| Missing tests/CI | Resolved: 376 library, 1 binary, 75 CLI (including PTY validation, safety abort, CPU→GPU continuation, preset dry-run and fan-status JSON semantics), 7 low-level installer tests, 1 language guard (460 total) and 66 isolated unified-installer flow tests |
| Version drift | Resolved: `Cargo.toml` is the single version source |
| Missing warning system | Resolved: shared severity-labelled warnings in CLI/TUI |
| Unvalidated profile catalogue | Resolved: compatibility evaluation, deterministic curves and supervisor apply/partial/blocked reporting |
| No persistent user curves | Resolved: versioned store, strict validation, atomic writes, backup/recovery, CLI/TUI editor and IPC CRUD |
| NBFC socket bypass | **Resolved on the host**: patched 0.5.4 socket `0600 root:root`, root-only policy, supervisor-only client model, ordinary-user denial verified |
| CLI read backend directly | Resolved: CLI/TUI use supervisor IPC; an ordinary-user NBFC denial can no longer be misreported as a backend outage |
| CLI/TUI read hardware directly | Resolved: sensor snapshots now come from the supervisor. `sensors list/watch`, status, diagnostics, fan status and hardware identity no longer collect `/sys` or run `nvidia-smi` in the client process. `hardware list` reads configuration files and `hardware validate` is a supervisor IPC method |
| Unbounded IPC connections | Resolved: concurrent handler threads are capped by `DaemonConfig::max_connections`; excess connections are closed without spawning a thread |
| Stale serialized freshness | Resolved: client reads recompute `health.fresh` from the supervisor sample timestamp, so an aged snapshot cannot claim freshness |
| Stale NBFC version cache | Resolved: revalidation on service restart/TTL and at write gates |
| Misleading readback label | Resolved: `hardware identity` reports the effective read range and its source; a write-range inference is described as a clamp source, not an invalid independent range |
| Runtime backend identification | Resolved read-only: supervisor inspects the NBFC command line, service configuration and open process descriptors; a known endpoint fd is `verified`, command/config evidence is `configured`, and an unobservable backend stays `unknown`. No EC open/write or `modprobe` is performed |
| Raw readback versus backend percentage | Resolved: `hardware identity` pairs configured read registers with the provisional EC channels that sample the same registers and reports `write_range_clamped` when the raw word is outside NBFC's effective range. This is register identity, not physical-fan identity or RPM |
| NBFC runtime on this host | `nbfc_service` PID 2393 reports `Using "ec_sys" as EmbeddedControllerType`; `ec_sys` is loaded with `write_support=Y` and refcount 1. The service command line/config selects no explicit backend, matching NBFC's ec_sys-first auto-probe |
| “Fan control” overstatement | Resolved: diagnostics separate manual-path eligibility, physical feedback and physical control validation |
| Profile refresh lost update | Resolved: profile evaluation uses the selection current at commit time |
| Installed-build drift | Resolved: the development host runs the published 2.4.0 release; installed binaries, unit and read-only health were recorded in `docs/HOST_INSTALL.md`. Deployment was performed outside the release-audit session |
| Manifest/lifecycle drift | Resolved: installer tests assert the unit's fail-safe ExecStopPost, absence of reset-ec, readiness/watchdog and sandbox properties |
| Feedback provenance | `hardware identity` reports `rpm_source` as `unavailable`, `hwmon`, or `provisional_ec`; neutral `rpm_channels` carry unit/raw metadata; uncalibrated EC words and unused ACPI/WMI paths are never presented as RPM |
| Acer RPM feedback | Current kernel exposes no Acer hwmon fan input; provisional EC tach candidates are reported with `Trust::Provisional`, `usable_for_feedback=false`, `verified_rpm=false` and no logical-fan mapping. `hardware identity` reports `rpm_source=provisional_ec`, not `hwmon`. No PWM/platform_profile; identity/calibration unverified |
| Feedback/regulation separation | `SensorHealth` exposes `usable_for_regulation`, `usable_for_feedback` and `physical_measurement`; the deprecated `usable_for_control` equals regulation eligibility. RPM can never enter temperature regulation, and feedback availability cannot enable control |
| Control-readiness model | `src/control.rs` produces `neosense.control_readiness/v1`: stage, runtime mode, profile mode, per-fan eligibility/blockers, and explicit control vs validation requirements. Hardware validation requires independent physical identity, RPM calibration and actuator-response instrumentation; the Acer state remains `hardware_unvalidated` because independent instrumentation is absent, while operator-attested CPU/GPU actuator response is recorded separately |
| Per-fan readiness | Every logical fan reports sources, mapping, temperature status, explicit policy blockers and unverified physical evidence; profiles report eligible/blocked fan indexes and role-filtered readiness |
| Calibration readiness gate | `neosense calibration readiness` and `diagnose.calibration_readiness` evaluate runtime backend, logical channels, physical identity, independent RPM, CPU/GPU telemetry, actuator semantics, recovery, 80/75 °C ceilings and the 50% floor. States are `blocked`, `partially_ready` and `ready_for_separately_authorized_test`; `informational_only=true` and `test_authorized=false` are always serialized. The evaluator never starts a test or writes hardware |
| Exact-model/ACPI telemetry research | DMI `Scala_TLS V1.17`, BIOS/EC `V1.17`/`1.10`, five ACPI `PNP0C0B` fan power-resource devices with binary state and no `_FIF`/`_FPS`/`_FSL`/`_FST` methods. Retail fan part numbers and a community `GH53G LA-L191P` board reference are leads only; exact fan assembly/pinout remains unknown |
| P1 calibration analysis | `neosense calibration analyze` performs pure offline regression/uncertainty analysis from paired external-reference and Acer RPM samples; relative airflow can never claim absolute RPM calibration |
| v1.0.1 control restoration | `auto`, `manual`, `set cpu|gpu|both` and the fixed cooling presets route through supervisor IPC. NBFC `set -f N -s`, `set -f N -a` and `set -a` semantics are preserved; multi-fan partial failures are explicit and automatic recovery is recorded |
| Primary cooling workflow | Silent 25%, Balanced 60% and Maximum 100% are explicit fixed logical targets with a 15-minute supervisor lease; legacy performance/turbo remain aliases. Preset dry-run output no longer claims a write. `auto` is documented as NBFC automatic software mode, not verified firmware handover |
| Closed-loop profile regulation | Supervisor `profile.apply` validates curves, takes ownership by returning all fans to automatic, preflights each fan, writes eligible targets, rechecks policy and verifies backend targets/modes. The sampler updates targets from fresh telemetry and stops at stale telemetry, lost access control, read-only/critical backend or missing fans. Control transitions and sampler writes are serialized. Manual/preset/all-automatic takeover ends the profile; per-fan automatic removes only that fan. Recovery is verified and recovery failure is a distinct fail-safe blocker |
| Fedora 44 VM installer validation | Official Fedora Cloud Base 44-1.7 qcow2 verified and booted under KVM; software-only archive install, unit/group/socket/service readiness, non-root CLI, upgrade, rollback and uninstall validated; official NBFC 0.5.4 RPM checksum-verified and installed, but NBFC service could not start without a laptop config/EC hardware. Backend-ready/physical control not tested |
| Release packaging | Reproducible Linux archive with package/version/target/profile/Rust metadata, checksum verification, prebuilt install/uninstall test and installer `--help`; two consecutive builds produced identical SHA-256 |
| TUI primary fan control | A shared visual language (compact header, filled active tab, rounded inset panels, one accent colour, `●`/`▶` markers) covers Dashboard, Fan Control, Presets, Profiles, Sensors, System and Diagnostics. Fan Control selects CPU/GPU/Both, edits bounded targets, adjusts by 5% and supports per-fan/all automatic. Mode selection (Auto/preset/profile) applies immediately; manual percentages still ask `y`/`n`. Dashboard derives the active mode from supervisor leases/profile state and shows lease and safety reasons. All writes use supervisor IPC |
| Fan discovery and index control | `fans list/show` expose typed backend indexes, logical names, physical identity `unknown`, configured temperature sources, mode/target/EC values, provisional EC tach candidates, independent manual/automatic/regulation capabilities and blockers. `set fan <index>` and `auto fan <index>` use the same supervisor policy. The GPU logical channel may use a trusted NVIDIA GPU temperature when NBFC declares `@CPU`; this is an explicit warning and does not establish physical identity |
| Capability separation | Fan inventory and control readiness keep discovery, physical identity, temperature source, RPM feedback, manual control, automatic control and hardware regulation validation distinct; RPM is never attached to a logical fan without verified mapping |
| GPU temperature policy | A GPU logical fan may use a fresh, trusted independent NVIDIA GPU die reading when NBFC declares `@CPU`; the mapping is explicit, warned, and does not establish physical identity. Missing/stale GPU telemetry keeps GPU control blocked |
| Provisional EC tachometer candidates | Read-only EC bytes `0x13/0x14` and `0x15/0x16` are exposed with `Trust::Provisional`, `usable_for_feedback=false` and no logical-fan mapping. The upstream community driver assembles `(low << 8) | high` and performs no unit conversion. They never enter regulation and are not validated RPM |
| Automatic-mode evidence | NBFC `AutoMode=true` is reported as its internal software policy. Raw EC mode codes (`0x04`/`0x0c` CPU, `0x10`/`0x30` GPU from the community driver) are exposed as separate provisional evidence. `fan.auto`, fan inventory and warnings distinguish `Verified`, `Unverified`, `Contradicted` and `BackendManual`; the installed config's manual EC modes are correctly reported as unverified, not as a contradiction |
| Interactive physical validation | `neosense fan validate` runs CPU→LEFT then GPU→RIGHT with separate operator authorizations. The supervisor owns a 50%/75%/100% sequence with a hard 50% experimental floor, continuous 80 °C CPU / 75 °C GPU ceilings, monotonic operation/step/session watchdogs, external-control invalidation, orderly shutdown cancellation and automatic recovery before observation. Backend target/mode verification, readback, EC evidence and physical response are explicit separate fields. Ordinary manual control still refuses 0%. Evidence is append-only JSONL with truncated-tail recovery and event-specific provenance; operator Yes does not set RPM calibration or physical identity automatically |
| Validation evidence inspection | `neosense validation history [--session ID]` exports supervisor-owned records read-only. The uninstaller preserves `/var/lib/neosense/validation` with custom profiles |
| Physical validation results | CPU logical channel previously accepted at 0/50/100 in supervisor session `validation-1794777264574-2`. GPU logical channel observed at 50/75/100 on 2026-11-17 with CPU automatic, 80/75 °C ceilings and verified Auto recovery. Operator-attested mapping CPU→LEFT / GPU→RIGHT; independent RPM/airflow calibration remains unverified. See `docs/VALIDATION_RESULTS.md` |
| acer_wmi compatibility | Upstream source and the running module audited; gaming WMI GUID present, AN515-57 missing from the DMI quirk list; no module parameter, reload or WMI call was made |

## Profiles

Profile states: `compatible`, `compatible with warnings`, `partial eligibility`, `preview only`, `unsupported`, `blocked for safety`. Compatibility is current policy eligibility, not physical validation.

- `safe-auto` is evaluated against NBFC availability, service state, CPU trust, GPU mapping, RPM and EC readback.
- `quiet`, `balanced` and `performance` have deterministic reference curves.
  They can be previewed locally or applied by the supervisor to eligible fans;
  blocked fans remain automatic.
- User-created custom curves are persistent dynamic profiles. They use the same
  regulation engine and can be applied to their selected roles when policy
  allows. The store is
  `/var/lib/neosense/profiles.json` (systemd `StateDirectory`), with
  `NEOSENSE_PROFILES_PATH` override and an unprivileged client fallback.
- The store provides versioned JSON, strict validation, an advisory write lock,
  atomic replace + fsync, a previous-generation backup, corruption recovery and
  explicit reset that preserves the corrupt input.
- An unavailable profile cannot be applied and explains why.
- Simulation never writes fan state. `profile apply` activates real
  supervisor-owned closed-loop regulation for eligible fans, with partial
  application and blocked-fan reporting.

## Safety policy

Manual control is refused when:

- NBFC < 0.5.4;
- service is read-only or critical;
- CPU/GPU data is missing, invalid, stale or insufficiently trusted;
- the fan sensor source is unknown, or the GPU fan has neither a GPU NBFC source nor a fresh trusted NVIDIA GPU temperature;
- speed is below the floor or above 100%;
- temperature is above the ceiling;
- confirmation is absent.

Manual lease: 15 minutes. Lease expiry, critical NBFC state, lost sensors, failed verification or backend loss trigger automatic recovery or fail-safe state.

## IPC and permissions

- Socket: `/run/neosense/neosensed.sock`, `root:neosense`, mode `0660`.
- Runtime directory: `/run/neosense`, `root:neosense`, mode `0750`.
- Group: `neosense`, created by the installer; membership requires a new login session.
- The user outside the group receives exit code 4 and a group/login hint.
- **Upstream NBFC still creates its socket as `0666`.** The host runs a reviewed 0600 patch with a root-only access list; the root supervisor is the only NBFC client. NeoSense's own group protects the supervisor socket, not NBFC's.

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
- That deployment installed build `897e757` (2026-11-17); installed binaries
  matched the verified release build byte-for-byte. Deployment used the
  supported installer, preserved state and validation evidence, and did not
  restart NBFC. A later 2.4.0 candidate was installed on 2026-11-18 and the
  published 2.4.0 release on 2026-11-19; see `HOST_INSTALL.md`.

## TUI

Seven views in everyday-priority order: Dashboard, Fan Control, Presets,
Profiles, Sensors, System, Diagnostics. The branded header shows a bold
ANSI Shadow-inspired `NEOSENSE` wordmark with the `LINUX THERMAL MANAGEMENT`
descriptor on terminals of at least 90×30, a compact two-row wordmark from
70×20, and a one-line wordmark below that. A strict printable-ASCII banner is
selected automatically on non-UTF-8 locales or with `NEOSENSE_ASCII=1`. A shared visual language uses a consistent header, a filled active tab,
inset rounded panels, one accent colour and `●`/`▶` markers for active and
selected states. Mode selection (Auto, preset or
profile) applies immediately; manual percentage changes still ask `y`/`n`.
Fan Control exposes CPU, GPU and a combined **Both fans** row. Presets render as
cards on wide terminals and a list on narrow ones. Profiles distinguish active,
selected and blocked states and support immediate apply, preview, deactivate and
`$EDITOR` custom-curve editing. Without a supervisor the TUI is read-only.

Rendering tests cover 20×10 through 160×48, including live resize, narrow
layouts, long fan names, header switching and help scrolling. A real pty smoke
test exercised all seven views and the help overlay against the installed
supervisor without issuing any fan command.

## Local validation

Confirmed: Acer Nitro AN515-57, BIOS V1.17, i5-11400H, RTX 3050 Laptop, Nobara 44, kernel 7.3.4, patched NBFC 0.5.4.

- CPU: coretemp package/cores; ACPI and x86_pkg thermal zones.
- GPU: NVIDIA via `nvidia-smi`; memory temperature N/A correctly unavailable.
- NVMe and Wi-Fi classified separately; Wi-Fi never promoted to GPU.
- NBFC: two logical fans with distinct register pairs (CPU 19/55, GPU 21/58), both declare `@CPU`; the GPU logical guard can use an independent trusted NVIDIA GPU die reading with an explicit warning. Physical fan identity is still unknown.
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
