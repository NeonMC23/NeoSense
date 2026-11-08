# NeoSense — TUI

Implemented with **Ratatui 0.30** and **Crossterm 0.29**. The TUI shares the Rust core and IPC protocol with the CLI.

```bash
neosense tui
```

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
- A pty 100×32 smoke test was performed; no sockets or hardware writes are used by TUI tests.
