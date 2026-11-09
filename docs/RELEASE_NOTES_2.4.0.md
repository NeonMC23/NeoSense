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
- 120 automated tests across the engine, CLI, daemon, TUI and installer.

## Safety

- Reference curves are simulation defaults, not validated hardware curves.
- GPU fan mapping remains unsuitable on the Acer Nitro AN515-57.
- No RPM or independent physical feedback is available.
- Manual and closed-loop hardware control remain disabled.
- A 2.4.0 simulation target is not a fan setting and is not RPM.
