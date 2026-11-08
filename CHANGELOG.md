# Changelog

All notable user-visible changes are documented here. The format follows
Keep a Changelog and the project uses semantic versioning.

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

### Changed

- TUI header is compact and structured; full IPC errors are confined to
  System/Diagnostics.
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

### Known limitations

- Manual fan control remains blocked on NBFC 0.5.3 and unvalidated hardware.
- NBFC 0.5.3 exposes its own socket as 0666; this upstream risk is documented.
- Closed-loop thermal profiles are preview-only.
- Root-level systemd installation and real group-session validation require
  sudo and are not complete in this environment.
- No physical fan-control validation has been performed.

## [2.2.0]

- Rust CLI/TUI migration, supervisor IPC, systemd watchdog and profile catalogue.
