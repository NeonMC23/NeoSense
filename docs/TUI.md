# NeoSense — TUI

Implemented with **Ratatui 0.30** and **Crossterm 0.29**. The TUI shares the Rust core and IPC protocol with the CLI.

```bash
neosense tui
```

## Header

Two compact rows show the application/version, hardware model, NBFC version and
supervisor state. Full IPC errors and socket details are confined to the System
and Diagnostics views; they never overflow or dominate the permanent header.

## Views

| View | Content |
|---|---|
| Dashboard | CPU, GPU, fans, target/EC, sensor source, mode, separate neutral RPM feedback, supervisor health, warnings |
| Sensors | Role, value, trust, status, provider, path |
| Fan Control | Discovered fan inventory, selected-fan detail, mode/target/EC/source/RPM, manual eligibility and blockers |
| Profiles | Catalogue, compatibility, preview targets, active profile, apply/deactivate and custom-curve editing |
| System | NBFC service, supervisor, IPC, warnings |
| Presets | Historical fixed targets: Silent 25%, Balanced 60%, Performance 80%, Turbo 100% |
| Diagnostics | NBFC, service, config, sensors, GPU mapping, RPM, socket, conflicts, control stage and blockers |

Control readiness is shown on the Dashboard and Diagnostics view with the
stage, runtime mode, per-fan eligible/blocked indexes and blocking requirement
details. The TUI reads supervisor state over IPC; it never reads hardware or
NBFC directly.

Fan Control uses the discovered fan inventory as its source of truth.
Up/Down selects a fan; `m` enters a bounded numeric target; `+`/`-` adjusts by
5%; `a` returns the selected fan to automatic; `A` returns all fans to
automatic. Every disruptive action requires `y`/`n` and calls supervisor IPC.
Blocked fans show the exact reason and are never given a manual target; profile application returns them to automatic.

Profiles distinguish preview/simulation from active regulation. Enter
simulates; `a` applies the profile to eligible fans through the supervisor;
`d` deactivates the active profile and returns fans to automatic. Applying a
profile can be partial: eligible fans are regulated, blocked fans are returned
to automatic and listed with reasons. Per-fan automatic removes only that fan from the profile; the profile continues for the remaining applied fans and ends when none remain. The selected curve is drawn as compact
percentage bars with its points, limits, hysteresis and rate limit. `e` edits a
custom curve in `$EDITOR`; `n` creates a new validated custom curve. The
dashboard shows the active profile and applied/blocked fans.

RPM feedback is shown as a separate read-only list using neutral names
(`Acer Fan 1/2`). It is not attached to NBFC CPU/GPU rows because no verified
physical mapping exists. RPM is physical feedback only; it is never a
temperature regulation input and does not enable control.

## Interaction

| Key | Action |
|---|---|
| `q`, Esc | Quit |
| `Tab`, `→` / `Shift-Tab`, `←` | Next/previous view |
| `1`–`7` | Direct view; `7` is fixed Presets |
| `Up`/`Down` | Select fan on Fan Control, profile on Profiles, preset on Presets |
| `m` | Fan Control: edit the selected fan's numeric target (bounded) |
| `+` / `-` | Fan Control: adjust the selected target by 5% |
| `a` | Fan Control: selected fan automatic (an active profile continues for its remaining fans); Profiles: apply selected profile |
| `A` | Return all fans to automatic and end any active profile, with confirmation |
| `d` | Profiles: deactivate active profile and return fans to automatic |
| `Enter` | Profiles: preview/simulate; safe-auto only explains that `a` applies it; Presets: apply fixed preset, with `y`/`n` |
| `e` | Edit the selected custom curve in `$VISUAL`/`$EDITOR`, then validate and save through IPC |
| `n` | Create a new custom curve from a validated template in `$VISUAL`/`$EDITOR` |
| `r` | Refresh |
| `?` | Help |

## Safety rules

- The TUI never writes NBFC or hardware directly.
- Manual targets, per-fan/all automatic, presets and profile apply/deactivate go through supervisor IPC and require confirmation where disruptive.
- Profile application activates real supervisor closed-loop regulation for eligible fans; blocked fans are returned to automatic. Selection alone remains simulation only. The Dashboard reports the authoritative safety state and any automatic-recovery failure. A failed profile is shown as inactive/failed, never as active.
- Fan Control exposes numeric manual targets; eligibility and blockers come from the supervisor policy. The GPU fan remains restricted while its source is `@CPU`.
- Without a supervisor, the TUI is read-only.
- EC values are labelled EC, never RPM.
- Missing values are `unavailable`/`?`, never `0`.

## Tests

- `TestBackend` renders at 40×20, 80×24, 100×32 and 160×48.
- Tab navigation, dynamic-profile preview/simulation, fixed-preset confirmation, help overlay and small-terminal behavior. No TUI test writes fan state.
- A pty 100×32 smoke test and rendered inspection at 40×20, 80×24 and 100×32 were performed; no sockets or hardware writes are used by TUI tests. Narrow layouts switch to compact tabs, tables and a shorter footer.

## Real terminal rendering evidence

The release TUI was launched in a pseudo-terminal and rendered through the
`pyte` terminal emulator at the actual dimensions. The six original views were captured
at 40×20, 80×24, 100×32 and 160×48 (24 screens), using the host's real sensor
data and the normal permission-denied supervisor state.

Observed fixes from that review:

- 40×20 uses compact tabs, three-column sensors, short trust/compatibility
  labels, a compact fan table and a single-line footer;
- 80×24 shows the full tab bar, scrolling hints on System/Diagnostics and the
  complete warning text after scrolling;
- 100×32 and 160×48 preserve readable columns and do not overflow;
- profile confirmation and action messages are visible in the footer;
- long diagnostics wrap and can be scrolled with `PgUp`, `PgDn`, `j`, `k` and
  `Home`;
- no empty dashboard is expected; an empty sensor set has an explicit message.

A faithful monochrome/colour PNG render of each captured terminal cell buffer
was generated for all 24 original-view screens and visually inspected. The images are not
committed, but they verify terminal geometry, colour/contrast attributes,
wrapping and hierarchy. A compact `j/k` hint was added to the System and
Diagnostics tab line at 40 columns. `TestBackend` tests additionally cover
empty states, narrow dimensions, confirmation prompts and scroll bounds.


## Custom curves

The Profiles view lists built-in dynamic curves followed by persistent user
curves, labelled `custom` (compact layouts append `*`). Pressing `e` on a custom profile writes its definition
to a temporary JSON file, suspends the TUI, opens the editor and then validates
the result. `n` creates a new template. Saving uses supervisor IPC
(`custom.update` / `custom.create`); the TUI never writes the store or fan state
directly. Invalid JSON, validation failures, name collisions and renames during
editing are reported in the footer without changing the stored profile.

If the supervisor is unavailable, the Profiles view remains read-only and the
edit keys explain that custom profiles are managed through the supervisor.
