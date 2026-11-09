# Changelog

All notable user-visible changes are documented here. The format follows
Keep a Changelog and the project uses semantic versioning.

## [2.4.0] — Unreleased

### Added

- Hardware-independent thermal policy engine with reference curves,
  interpolation, floor/ceiling clamping, hysteresis, rate limiting, sensor
  freshness and per-fan mapping.
- Local `profile preview`, supervisor `profile.select`/`profile.get`/
  `profile.clear` simulation APIs, and TUI preview targets/selection. Simulation
  never performs a fan write.
- Development version decision: 2.3.0 remains frozen; 2.4.0 is unreleased.

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
