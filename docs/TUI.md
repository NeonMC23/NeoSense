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
| Dashboard | CPU, GPU, fans, target/EC, sensor source, mode, supervisor health, warnings |
| Sensors | Role, value, trust, status, provider, path |
| Fans | Index, name, role, mode, requested, target, EC, source |
| Profiles | Catalogue, compatibility, reasons; only `safe-auto` is applicable |
| System | NBFC service, supervisor, IPC, warnings |
| Diagnostics | NBFC, service, config, sensors, GPU mapping, RPM, socket, conflicts |

## Interaction

| Key | Action |
|---|---|
| `q`, Esc | Quit |
| `Tab`, `→` / `Shift-Tab`, `←` | Next/previous view |
| `1`–`6` | Direct view |
| `Up`/`Down` | Select profile in Profiles view |
| `Enter` | Preview/apply selected profile, with `y`/`n` confirmation |
| `r` | Refresh |
| `a` | Return to NBFC automatic mode, with confirmation |
| `?` | Help |

## Safety rules

- The TUI never writes NBFC or hardware directly.
- `fan.auto` and `safe-auto` profile application go through supervisor IPC and require confirmation.
- No manual fan-speed command is exposed in the TUI.
- Without a supervisor, the TUI is read-only.
- EC values are labelled EC, never RPM.
- Missing values are `unavailable`/`?`, never `0`.

## Tests

- `TestBackend` renders at 40×20, 80×24, 100×32 and 160×48.
- Tab navigation, profile selection/confirmation, help overlay and small-terminal behavior.
- A pty 100×32 smoke test and rendered inspection at 40×20, 80×24 and 100×32 were performed; no sockets or hardware writes are used by TUI tests. Narrow layouts switch to compact tabs, tables and a shorter footer.

## Real terminal rendering evidence

The release TUI was launched in a pseudo-terminal and rendered through the
`pyte` terminal emulator at the actual dimensions. All six views were captured
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

No image screenshots were captured or committed. Colour/contrast was not
reviewed as pixels; the review is textual terminal-screen geometry, content,
wrapping and hierarchy. `TestBackend` tests additionally cover empty states,
narrow dimensions, confirmation prompts and scroll bounds.
