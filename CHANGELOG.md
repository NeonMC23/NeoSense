# Changelog

All notable user-visible changes are documented here. The format follows
Keep a Changelog and the project uses semantic versioning.

## [2.4.0] — Unreleased

### Added

- Hardware-independent thermal policy engine with reference curves,
  interpolation, floor/ceiling clamping, hysteresis, rate limiting, sensor
  freshness and per-fan mapping.
- Local `profile preview`, supervisor `profile.select`/`profile.get`/
  `profile.clear` simulation APIs, continuous re-evaluation and TUI preview
  targets/selection. Simulation never performs a fan write.
- Development version decision: 2.3.0 remains frozen; 2.4.0 is unreleased.
- Persistent custom thermal curves: versioned JSON, strict validation, atomic
  writes, write lock, backup and corruption recovery, supervisor IPC CRUD,
  CLI lifecycle commands and TUI `$EDITOR` editing. Curves are validated and
  can be applied by the supervisor to policy-eligible fans.
- The service owns `/var/lib/neosense` via `StateDirectory`; uninstall preserves
  user `profiles.json`.
- Added read-only `neosense hardware identity` and diagnostics for NBFC fan
  register pairs, sensor sources and readback validity. Invalid NBFC readback
  ranges are surfaced instead of treating EC `CurrentSpeed` as a percentage.
- Added read-only NBFC runtime-backend identification. The supervisor inspects
  the service command line, service configuration and open process descriptors
  for the EC endpoint; an open `ec_sys` descriptor is reported as verified,
  while an unobservable process is reported as unknown rather than inferred.
  The evidence is exposed by `status`, `fans list`, `hardware identity` and
  `diagnose`.
- `hardware identity` now carries per-fan `readback_evidence` comparing the raw
  EC word in the same read register with NBFC's effective range and
  `CurrentSpeed`. A raw word outside the write-derived range is reported as
  `write_range_clamped`, not as a measured speed. `diagnose` emits a matching
  `nbfc.backend` check and a per-fan raw-readback warning.
- Added NBFC socket access-control classification. Group/world-writable
  sockets are reported as security warnings and block manual, preset and
  closed-loop hardware actions; automatic recovery remains available.
- Restored v1.0.1 fixed presets: Silent 25%, Balanced 60%, Performance 80%,
  Turbo 100%, applied through supervisor preflight, backend verification and
  multi-fan safety leases. TUI Presets view and CLI `preset` commands added.

### Added

- Interactive physical validation (`neosense fan validate`) runs CPU→LEFT and
  GPU→RIGHT with separate explicit authorizations. The supervisor owns the
  bounded 50%/75%/100% sequence with a hard 50% experimental floor, continuous
  CPU 80 °C / GPU 75 °C ceilings, monotonic operation/step/session watchdogs,
  external-control invalidation, orderly shutdown cancellation and automatic
  recovery before observation. Evidence is append-only JSONL with explicit target/mode,
  readback, EC and physical-response fields, actual hold timing, truncated-tail
  recovery and event-specific provenance. Ordinary manual control still refuses
  0%; experimental validation never commands below 50%.
- `neosense validation history` exposes supervisor-owned validation records
  read-only. Uninstall preserves validation evidence with custom profiles.
- Fixed false validation aborts at the former 0% → 50% transition: backend status
  reads are serialized with target writes and the supervisor refreshes status
  after a successful write before re-enabling drift detection. Validation
  evidence now records explicit target/mode/readback/physical-response fields,
  actual hold timing and abort reasons; legacy `verified` is target/mode-only
  and is no longer emitted as a success claim for validation steps.
- Added PTY end-to-end coverage for a visible safety abort, recovery reporting,
  successful CPU→GPU continuation with separate session evidence, and declining
  continuation without starting the GPU workflow.
- Polished the everyday control workflow. The TUI now puts Fan Control and
  Presets immediately after Dashboard, adds Dashboard shortcuts `s` Silent,
  `b` Balanced, `M` Maximum and `A` Auto, and adds a Both-fan manual target.
  The TUI target editor now enforces the real 20–100% manual floor instead of
  suggesting 0–100%. CLI help includes a concise quick-start block and documents
  confirmation/dry-run behavior.
- Recorded physical validation results: the CPU logical channel remains
  accepted at 0/50/100 from the earlier supervisor session; the GPU logical
  channel was observed at 50/75/100 on 2026-11-17 with the CPU fan automatic,
  80/75 °C ceilings and verified Auto recovery. Operator-attested mapping is
  CPU→LEFT and GPU→RIGHT; independent RPM/airflow calibration remains
  unverified. See `docs/VALIDATION_RESULTS.md`.
- Clarified that `auto` restores the NBFC temperature-dependent software curve,
  which is the laptop's established normal automatic behavior and the same
  recovery used after the validated CPU/GPU tests. The readiness recovery
  requirement now accepts confirmed backend automatic recovery; EC mode
  remaining NBFC-managed is expected for this configuration.
- Simplified the primary cooling presets to Silent 25%, Balanced 60% and
  Maximum 100%. `performance` (80%) and `turbo` (100%) remain legacy aliases.
  Presets are explicit fixed logical targets with a 15-minute supervisor lease,
  not hidden temperature curves.
- Fixed `preset apply --dry-run` human output, which previously claimed the
  preset was applied and targets confirmed even though no write occurred.
- Clarified the primary CLI workflow and documented that `auto` restores NBFC
  automatic software control, while genuine firmware automatic handover remains
  unverified and requires a separately authorized reset/service path.
- Added read-only `neosense calibration readiness` and
  `neosense.calibration_readiness/v1`. The evaluator reports `blocked`,
  `partially_ready` or `ready_for_separately_authorized_test` for runtime
  backend, logical channels, physical identity, independent RPM, CPU/GPU
  telemetry, actuator semantics, recovery, temperature ceilings and the 50%
  experimental floor. It always sets `test_authorized=false`; exit 0 is still
  informational and never authorizes a test. `diagnose --json` embeds the same
  object and TUI diagnostics show the state.
- Documented the five ACPI `PNP0C0B` fan devices on this machine. They expose
  binary cooling/power-resource state (`max_state=1`) and no `_FIF`/`_FPS`/
  `_FSL`/`_FST` speed methods; they are not RPM and do not prove five physical
  fans.
- `hardware identity` reports provisional EC channels as
  `rpm_source: "provisional_ec"` rather than `"hwmon"`; raw units and values
  remain explicit and are not treated as verified RPM.
- Fan status JSON now serializes provisional EC tach words as `unit: "raw"`
  with `verified_rpm: false`, preserves raw unit/value/provenance metadata, and
  adds `fan_semantics` clarifying that NBFC `RequestedSpeed` is historical in
  automatic mode while `TargetSpeed` is the active software target. The text
  table labels `(last)` and `(auto)` consistently.
- Automatic-mode evidence is separated from NBFC's software policy. Raw EC
  fan-mode codes are exposed as provisional evidence, and fan inventory,
  warnings, CLI/TUI diagnostics and `fan.auto` distinguish EC-verified,
  unverified, contradicted and backend-manual states. NBFC `AutoMode=true`
  is no longer presented as proof of firmware automatic operation.
- GPU logical-channel temperature policy now accepts a fresh, trusted
  independent NVIDIA GPU die reading when NBFC declares a non-GPU source such
  as `@CPU`. The policy emits an explicit warning, never substitutes CPU
  temperature and never claims physical fan identity.
- Read-only Acer EC tachometer candidates (`0x13/0x14` and `0x15/0x16`) are
  exposed as provisional, unmapped measurements. They are never usable as
  feedback or regulation evidence until units and physical mapping are
  independently validated.

### Fixed

- Closed-loop profile application now serializes with the sampler and all
  manual/preset/automatic transitions, so an in-flight regulation write cannot
  overwrite a newer control request.
- Applying a profile takes ownership of unrelated manual/preset fans; per-fan
  automatic now removes only the selected fan while the profile continues for
  the remaining applied fans.
- Profile application re-checks policy after writes and stops at stale
  telemetry, lost NBFC access control, read-only/critical backend state or
  missing manual capability.
- Automatic recovery is verified and reported truthfully. Failed recovery is
  exposed as a fail-safe blocker instead of claiming fans are automatic.
- Active custom profile edits/deletes/resets deactivate regulation before
  changing persistence; invalid edits leave the active profile untouched.
- CLI/TUI state distinguishes active, simulated, failed and inactive profiles
  and shows the supervisor safety state.
- TUI Enter on safe-auto no longer writes; only `a` applies it.

### Changed

- Added a unified Linux installation entry point, `neosense.sh`, with modular
  host/distribution detection, DNF/APT/Pacman/Zypper adapters, versioned
  dependency checks, explicit package-install authorization, backend detection
  and conflict reporting, transactional delegation to `install.sh`, and a
  read-only capability verification. A missing or incompatible backend stops the
  default installation before software changes; `--software-only` is explicit.
  The installer never changes NBFC configuration, stops another controller or
  writes fan state.
- Added isolated installer-flow tests for Nobara/Fedora, Debian/Ubuntu, Arch,
  openSUSE and unknown hosts, missing dependencies, old versions, backend
  states, conflicts, dry-run install/uninstall and non-interactive safeguards.
- Added `docs/INSTALL.md` with the dependency matrix, distribution validation
  status and recovery rules; the release archive now bundles the unified entry
  point and helper modules.
- Completed a genuine Fedora 44 KVM VM software-only installation validation:
  archive install, systemd unit/socket/permissions, non-root CLI, upgrade,
  injected-failure rollback and uninstall. The official NBFC 0.5.4 Fedora 44
  RPM was checksum-verified and installed, but NBFC could not start without a
  laptop configuration and EC hardware.
- Finalized the terminal identity as the ANSI Shadow-inspired `NEOSENSE` banner
  alone with the `LINUX THERMAL MANAGEMENT` descriptor, version, supervisor,
  NBFC and host; the experimental ASCII monogram was removed. Large terminals
  use a telemetry rail, medium terminals a compact two-row wordmark, and small
  terminals a one-line wordmark. A strict printable-ASCII banner is used on
  non-UTF-8 locales or with `NEOSENSE_ASCII=1`.
- Fan panels now report the concrete discovery error (such as an inactive NBFC
  service) instead of claiming the supervisor is unreachable when it is not.
- Polished the Linux release pipeline: `install.sh` and `uninstall.sh` now
  document their options with `--help`, the installer rejects unsupported
  custom runtime/state paths for real systemd installs with an explicit test
  seam, and next-step hints are only printed when they still apply. Release
  archives record package, target, profile, Rust and Cargo versions in
  `BUILD-INFO`; the archive is reproducible with the recorded toolchain. CI now runs on
  every pushed branch.
- Release-audit hardening: runtime/state overrides cannot name standard system
  directories, non-numeric boolean switches are rejected, verification resolves
  paths after option parsing, unrecognised NBFC status output is an error
  instead of an empty healthy backend, daemon store errors use the documented
  exit codes, `a` on Profiles means NBFC automatic mode, and the stop timeout
  lets the `ExecStopPost` fail-safe finish.
- Operational hardening: a failed automatic recovery is retried twice with a
  short bounded backoff, reported through supervisor health and the
  `recovery` status object, and stops the watchdog instead of appearing
  healthy. Custom-profile commands no longer write the local fallback when the
  supervisor store is inaccessible or rejects the operation; status output
  names the active store scope. `hardware validate` waits up to 35 seconds for
  its 30-second read-only backend analysis instead of failing at 10 seconds.
- Redesigned the Ratatui interface around a consistent visual language: a
  compact header, filled active tab, inset rounded panels, one accent colour and
  explicit `●`/`▶` active/selected markers. The Dashboard now prioritises the
  current mode, both fan states and temperatures; Fan Control adds a selectable
  **Both fans** row; Presets render as mode cards on wide terminals; Profiles
  distinguish active, selected and blocked states.
- Mode selection is now immediate. Applying Auto, Silent, Balanced, Maximum or a
  thermal profile no longer asks for a second `y`/`n` confirmation. Manual
  percentage changes still require confirmation, and all supervisor safety
  gates are unchanged. Dashboard shortcuts are `s` Silent, `b` Balanced,
  `m` Maximum and `a` Auto.
- The TUI now derives the displayed mode from supervisor leases and profile
  state, shows the remaining lease, and reports safety reasons on the Dashboard
  instead of a separate control-readiness paragraph.
- CLI human output uses consistent aligned tables and labels for `status`,
  `preset list`, `profile list`, `profile show` and custom-profile listings.
  JSON output and command semantics are unchanged.
- Rebuilt `README.md` as a product landing page and refreshed `docs/TUI.md`,
  `docs/PROFILES.md` and the 2.4.0 release notes to match the current
  implementation.
- The CLI and TUI now obtain the sensor snapshot from the supervisor over IPC
  instead of reading `/sys` or running `nvidia-smi` in the client process.
  Without the supervisor, live sensor data is explicitly unavailable.
- `hardware list` reads configuration files directly; `hardware validate` is a
  supervisor IPC method, so no CLI path invokes the NBFC client.
- Supervisor IPC concurrency is bounded; excess connections are rejected
  instead of spawning unbounded handler threads.
- Serialized sensor freshness is recomputed from the sample timestamp, so an
  aged supervisor snapshot cannot claim to be fresh.

### Added

- `neosense control readiness` and `diagnose.control`: a structured,
  backend-independent readiness assessment with per-fan eligibility and
  separate control versus validation requirements. Hardware validation
  requires explicit physical-identity, RPM-calibration and actuator-response
  evidence; the Acer machine remains unvalidated.
- `neosense hardware validate` now returns the typed, actionable
  `analysis_unavailable` limitation when NBFC's optional `acpiexec`
  (`acpica-tools`) dependency is absent, instead of a generic backend failure.
  No fan write is attempted.
- The TUI dashboard now shows control readiness, runtime mode and per-fan
  eligible/blocked indexes.
- Per-fan readiness now carries explicit blockers, NBFC sources, temperature
  status and unverified physical evidence. Profiles report eligible/blocked
  fan indexes and role-filtered readiness.
- Added `neosense calibration analyze --input FILE`: pure offline P1
  regression, confidence-interval and acceptance analysis for paired external
  reference/Acer RPM samples. It never accesses hardware and cannot validate
  physical identity or actuator response.
- Restored the v1.0.1 control surface: `neosense auto`, `manual`,
  `set cpu|gpu|both`, `turbo`, `performance`, `balanced` and `silent`.
- Added supervisor IPC operations for multi-fan fixed targets, manual mode at
  current targets, and per-fan automatic mode. Multi-fan results distinguish
  requested, backend-accepted and backend-reported values; backend failures
  return structured partial results and request automatic recovery.
- Added the NBFC `set -f N -a` per-fan automatic backend operation.
- Added typed fan discovery: `neosense fans list` and `fans show <index>` report
  stable backend indexes, configured/display names, logical roles, physical
  identity confidence, temperature sources, mode, requested/target/EC values,
  RPM provenance (or explicit unmapped N/A), manual/automatic/regulation
  capability and blockers.
- Added index-based control: `neosense set fan <index> <percent>` and
  `neosense auto fan <index>`. CPU/GPU aliases remain for compatibility.
- GPU fixed manual control remains restricted because its NBFC source is
  `@CPU` and no trusted GPU thermal guard exists; the inventory explains this
  rather than silently substituting GPU temperature.
- Supervisor-owned closed-loop profile regulation: `profile.apply` validates a
  built-in/custom curve, preflights per-fan eligibility, writes and verifies
  eligible fans, updates targets from fresh telemetry on the sampler loop and
  fails closed to automatic on sensor/backend loss. Blocked fans remain
  automatic and are reported as partial application.
- TUI Fan Control now selects discovered fans, edits numeric targets, supports
  per-fan/all automatic mode and displays blockers. Profiles supports real
  apply/deactivate alongside simulation/preview and custom-curve editing;
  Dashboard shows the active profile and applied/blocked fans.
- Manual/preset/per-fan-auto takeover deactivates an active profile first, so
  only one control policy owns a fan.
- Fixed manual-at-current-target selection to use NBFC `TargetSpeed`, not the
  `RequestedSpeed=0.0` placeholder reported in automatic mode. The same fix
  applies to `fan hold`.

### Fixed

- API error details are preserved when the CLI maps a supervisor error to a
  numeric exit code.
- Added repeated and concurrent IPC status-query soak tests proving stable
  fan/sensor state and zero writes.
- Added regression coverage that RPM feedback never upgrades a closed-loop
  profile to hardware compatibility and that one provider failure does not
  invalidate unrelated trusted sensors.

## [2.3.0] — 2026-11-09

### Added

- Shared warning and severity model used by CLI and TUI.
- Profile compatibility evaluation: compatible, compatible with warnings,
  preview only, unsupported and blocked for safety.
- TUI System view and profile selection/confirmation through supervisor IPC.
- English product UI and documentation.
- Shell completions command.
- Categorized diagnostics with machine-readable summary and grouped checks.
- Reproducible release archive script, completions, BUILD-INFO and checksums.
- Installer backup/verification/rollback foundations and state directory.
- Completion generation command for bash, zsh and fish.
- `sensors watch --once`, validated interval input and NDJSON output for
  script-safe sampling.
- Stable `schema` fields on sensor, profile, hardware and diagnostic reports.
- All five advertised completion shells are generated and packaged.
- Prebuilt release archive installation: `install.sh` uses bundled binaries
  when present and falls back to a Cargo source build.
- Isolated installer and release-archive tests covering fresh install,
  upgrade/rollback, uninstall and prebuilt archive installation.
- Disposable-container systemd validation: READY, socket permissions, group
  access, watchdog, clean stop, SIGKILL recovery and ExecStopPost.

### Changed

- TUI header is compact and structured; full IPC errors are confined to
  System/Diagnostics. Narrow terminals use compact tabs, tables and cards.
- Release archives record the full source commit and commit build time, and
  are created with deterministic archive ordering and timestamps.
- Supervisor states are distinct: healthy, degraded, not running,
  permission denied, stale socket and protocol error.
- `service status` reports distinct exit codes for these states.
- Diagnostic output consolidates the NBFC manual-control limitation.
- CLI fan output uses aligned columns and never conflates EC percentages
  with RPM.

### Fixed

- TUI header overload and long supervisor error leakage.
- Missing vs permission-denied supervisor classification.
- Repeated NBFC version warnings in diagnostics.
- Remaining French product strings in fan modes, sensor errors, NBFC parser
  errors, watchdog messages and help text; a regression test scans source for
  known non-English fragments.
- TUI System/Diagnostics content can be scrolled with PgUp/PgDn, j/k and Home;
  narrow footers no longer clip the scroll hint.
- Empty sensor/fan states are explicit instead of blank tables.
- Installer now verifies both binary versions, refuses a missing systemd unit,
  rolls back on `systemctl daemon-reload` failure, and handles INT/TERM with
  rollback.
- IPC reads are bounded before allocation; a custom `--socket` no longer
  changes permissions on an existing shared parent directory.
- Multi-fan `fan hold` resolves every target before the first write and
  requests automatic recovery if a later write fails; `--dry-run` never writes
  or invokes recovery.
- Validated a genuine historical 2.2.0 -> 2.3.0 upgrade and an injected
  daemon-reload rollback in an isolated systemd container.
- `neosense tui --no-color` is rejected explicitly instead of silently ignoring
  the option; the TUI keeps its fixed semantic palette.
- Upgrades of an active supervisor now restart it, verify READY/socket/IPC,
  and roll back plus restore the previous service if restart verification
  fails. `--no-restart` opts out for manual control.
- Supervisor polling now caches the immutable NBFC version and periodically
  refreshes sensor configuration instead of spawning duplicate `nbfc`
  processes every sampling interval.
- Compact System/Diagnostics tabs advertise `j/k` scrolling; the installer
  verifies a restarted supervisor on custom runtime paths using the configured
  socket.
- Compact TUI labels no longer clip trust/compatibility values at 40/80 columns.
- IPC responses are checked against the request id; accepted server connections
  have read/write timeouts so a stalled client cannot pin a thread indefinitely.
- Installer and uninstaller reject unsafe absolute paths (`/`, `.`, `..`,
  duplicate separators) before any file operation.
- Uninstaller refuses to delete NeoSense files if the supervisor cannot be
  stopped/disabled or systemd cannot reload the unit.
- Narrow-terminal TUI clipping for tabs, sensors, fans, profiles and footer.
- Prebuilt release archive installation, relative SHA-256 manifest and a
  failing full-commit check in the archive test.

### Known limitations

- Manual fan control remains blocked on NBFC 0.5.3 and unvalidated hardware.
- NBFC 0.5.3 exposes its own socket as 0666; this upstream risk is documented.
- Closed-loop thermal profiles are preview-only.
- Root-level systemd installation and real group-session validation require
  sudo and are not complete in this environment.
- No physical fan-control validation has been performed.

## [2.2.0]

- Rust CLI/TUI migration, supervisor IPC, systemd watchdog and profile catalogue.
