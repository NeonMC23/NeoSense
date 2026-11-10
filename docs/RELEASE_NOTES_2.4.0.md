# NeoSense 2.4.0 — development notes

**Status:** development; not released. The 2.3.0 archive remains frozen and
unpublished. This version adds profile simulation, not real closed-loop
hardware control.

## Added

- Hardware-independent thermal policy engine with deterministic reference
  curves, interpolation, hysteresis, rate limiting, staleness checks and
  per-fan capability evaluation.
- `neosense profile preview <name>` for local no-write target calculation.
- Supervisor profile simulation APIs and TUI profile targets/selection.
  These require the 2.4.0 supervisor; a 2.3.0 daemon returns `unknown method`.
- Restored historical fixed presets: Silent 25%, Balanced 60%, Performance 80%,
  Turbo 100%, with supervisor preflight, multi-fan safety leases and TUI
  Presets view. They remain blocked on the current unverified hardware.
- Persistent custom dynamic profiles: versioned JSON, strict validation,
  atomic writes and fsync, advisory write lock, previous-generation backup,
  corruption recovery, explicit reset, v0/legacy migration, supervisor IPC
  CRUD, CLI create/show/update/edit/duplicate/rename/delete/validate, and TUI
  `$EDITOR` editing. See `docs/CUSTOM_PROFILES.md`.
- The systemd unit owns `/var/lib/neosense` through `StateDirectory`; uninstall
  preserves `profiles.json` rather than deleting user curves.
- 157 automated tests across the engine, persistent custom profiles, historical presets, CLI, daemon, TUI and installer.

## Safety

- Reference curves are simulation defaults, not validated hardware curves.
- GPU fan mapping remains unsuitable on the Acer Nitro AN515-57.
- No RPM or independent physical feedback is available.
- Manual and closed-loop hardware control remain disabled.
- A 2.4.0 simulation target is not a fan setting and is not RPM.
