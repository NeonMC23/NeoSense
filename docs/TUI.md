# NeoSense — Terminal UI

The TUI is built with **Ratatui 0.30** and **Crossterm 0.29**, shares the Rust
core and IPC protocol with the CLI, and never touches hardware or NBFC directly.

```bash
neosense tui
```

## Design

The interface is intentionally restrained: a compact header, a single tab bar,
inset rounded panels consistently used across views, one accent colour (cyan)
for focus and brand, green for automatic/active states, yellow for attention and
red for failures. State is never communicated by colour alone — active and
selected rows also carry `●` and `▶` markers, and mode tags use filled labels.

Seven views, in everyday-priority order:

| View | Content |
|---|---|
| **Dashboard** | Current mode and tag, backend targets, both fan states, temperatures, safety and active warnings |
| **Fan Control** | Selectable CPU / GPU / Both rows, target bar, mode, EC readback, source, temperature and lease |
| **Presets** | Silent 25%, Balanced 60% and Maximum 100% as selectable cards (list on narrow terminals) |
| **Profiles** | Catalogue with active/selected markers, compatibility, live preview target and curve detail |
| **Sensors** | Role, value, trust, status, provider and path |
| **System** | Services, backend identity, IPC socket, custom store and warnings |
| **Diagnostics** | Control stage, eligible/blocked fans, calibration prerequisites and grouped checks |

The Dashboard answers three questions at a glance: *which mode is active*,
*what are both fans doing*, and *is anything wrong*. Detailed diagnostics stay
in their own views.

## Header and identity

The identity is typographic. The primary mark is an ANSI Shadow-inspired
`NEOSENSE` wordmark built from solid blocks and box-drawing corners:

```text
███╗   ██╗ ███████╗  ██████╗  ███████╗ ███████╗ ███╗   ██╗ ███████╗ ███████╗
████╗  ██║ ██╔════╝ ██╔═══██╗ ██╔════╝ ██╔════╝ ████╗  ██║ ██╔════╝ ██╔════╝
██╔██╗ ██║ █████╗   ██║   ██║ ███████╗ █████╗   ██╔██╗ ██║ ███████╗ █████╗
██║╚██╗██║ ██╔══╝   ██║   ██║ ╚════██║ ██╔══╝   ██║╚██╗██║ ╚════██║ ██╔══╝
██║ ╚████║ ███████╗ ╚██████╔╝ ███████║ ███████╗ ██║ ╚████║ ███████║ ███████╗
╚═╝  ╚═══╝ ╚══════╝  ╚═════╝  ╚══════╝ ╚══════╝ ╚═╝  ╚═══╝ ╚══════╝ ╚══════╝
```

### Responsive tiers

| Terminal | Header |
|---|---|
| ≥ 118×30 | Full banner with a right-hand telemetry rail: version, descriptor, supervisor, NBFC and host |
| 90–117 columns, ≥ 30 rows | Full banner with a single status line below |
| ≥ 70×20 | Compact two-row wordmark with version, supervisor, NBFC and host |
| Smaller | One-line `NeoSense` wordmark, machine model and supervisor state |

The banner is not a splash screen: it stays in the header on every view, above
the tab bar. On the three tested sizes it consumes 7 rows at 120×40, 3 rows at
80×24 and 2 rows at 60×18.

### Unicode and ASCII fallback

- Unicode is the default when the locale is UTF-8 (`LC_ALL`, `LC_CTYPE` or
  `LANG` contains `UTF-8`/`utf8`). This is the intended presentation.
- A strict printable-ASCII fallback is used when the effective locale is not
  UTF-8, or when `NEOSENSE_ASCII=1` (also `true`, `yes`, `on`) is set.
- The fallback uses `#`, `+`, `|` and `=` for the same 76×6 banner, so the
  identity remains recognizable without Unicode block fonts. The rest of the
  TUI continues to use standard terminal box drawing and arrows.

```text
###+   ##+ #######+  ######+  #######+ #######+ ###+   ##+ #######+ #######+
####+  ##| ##+====+ ##+===##+ ##+====+ ##+====+ ####+  ##| ##+====+ ##+====+
##+##+ ##| #####+   ##|   ##| #######+ #####+   ##+##+ ##| #######+ #####+
##|+##+##| ##+==+   ##|   ##| +====##| ##+==+   ##|+##+##| +====##| ##+==+
##| +####| #######+ +######++ #######| #######+ ##| +####| #######| #######+
+=+  +===+ +======+  +=====+  +======+ +======+ +=+  +===+ +======+ +======+
```

All artwork is made from standard terminal cells. No image protocols, external
fonts or graphical dependencies are used, and colour is decorative: the
wordmark is legible in monochrome.

## Cooling modes

Mode selection is **immediate**. Selecting Auto, a preset or a profile applies
it through the supervisor without an extra `y`/`n` prompt, matching the CLI's
`--yes` model.

| Key | Context | Action |
|---|---|---|
| `s` | Dashboard | Apply Silent (fixed 25%) |
| `b` | Dashboard | Apply Balanced (fixed 60%) |
| `m` | Dashboard | Apply Maximum (fixed 100%) |
| `a` | Dashboard / other views | Return all fans to NBFC automatic mode |
| `A` | Any view | Return all fans to automatic and end an active profile |

The active mode is shown as a filled tag (`AUTO`, `MANUAL`, `PROFILE`,
`FAILSAFE`) plus a `●` marker in the mode selector. Presets are inferred from
the supervisor's active fixed-target leases, so the indicator reflects the
supervisor state rather than a local memory.

## Fan Control

The table contains one row per discovered logical fan and a final **Both fans**
row. Up/Down selects any of them; the selected row is marked with `▶` and the
detail panel names it.

| Key | Action |
|---|---|
| `↑` / `↓` | Select CPU, GPU or Both |
| `m` | Set the selected target (or both targets on the Both row) |
| `b` | Set one target for both fans |
| `+` / `-` | Adjust the selected target by 5% |
| `a` | Return the selected fan (or both) to automatic |
| `A` | Return all fans to automatic |

Manual targets are bounded to **20–100%**. The editor rejects 0% and out-of-range
input before any IPC request is made. Manual writes still ask `y`/`n` because
they increase a fan above its current target.

The detail panel separates:

- requested target, backend-accepted target and EC readback;
- backend mode (`auto`/`manual`) and per-fan automatic evidence;
- configured temperature source, sensor trust and freshness;
- measured RPM only when a verified mapping exists (otherwise `unavailable`);
- the remaining lease, or `no manual lease` in automatic mode;
- policy blockers and warnings.

Applying a manual target or profile ends an active profile. Returning one fan to
automatic removes only that fan; the profile continues for the remaining applied
fans until none remain.

## Presets

Wide terminals show three cards; narrow terminals show an aligned list. Each
entry displays an `ACTIVE`/`SELECTED` marker, CPU and GPU targets and the fixed
target state. `Enter` applies immediately. Presets are fixed logical targets —
not temperature curves, not RPM — with a 15-minute lease, after which the
supervisor restores NBFC automatic mode.

## Profiles

| Key | Action |
|---|---|
| `↑` / `↓` | Select a profile |
| `Enter` | Apply the selected profile immediately |
| `p` | Preview targets without writing |
| `d` | Deactivate the active profile and return fans to automatic |
| `e` | Edit the selected custom curve in `$VISUAL`/`$EDITOR` |
| `n` | Create a custom curve from a validated template |

Active and selected states are distinct: `●` marks the profile the supervisor is
running, `▶` marks the list focus. The detail panel draws the curve as
percentage bars with its floor, ceiling, hysteresis and rate limit, and reports
eligible/blocked fans with reasons.

Profile application can be partial by design: eligible fans are regulated,
blocked fans stay automatic. Selecting a profile for preview (`p`) sends
`profile.select`, which never writes fans. A failed profile is shown as
inactive, never as active.

## Other views

- **Sensors** — one aligned table; trust is colour-coded and also labelled.
- **System** — grouped Services, Backend, Profiles and Warnings sections.
- **Diagnostics** — summary, control stage, calibration prerequisites, critical
  issues and grouped checks. It always reports `test authorized false`.

Long views scroll with `j`/`k`, `PgUp`/`PgDn` and `Home`; the footer advertises
scrolling when relevant. The help overlay scrolls the same way and resets to the
top when reopened.

## Small terminals

- The Dashboard falls back to a compact summary below 50 columns or 16 rows.
- Fan Control drops the role/EC/RPM columns below 88 columns and hides the
  detail panel when there is no vertical room.
- Presets switch from cards to a list below 96 columns.
- Profiles switch to name/availability/target columns below 88 columns.
- The branded header collapses to the compact two-row wordmark at medium sizes and to one line below 70 columns or 20 rows.
- The tab bar switches to a compact index below 62 columns.

Rendering is covered by `TestBackend` tests at 20×10, 24×8, 30×12, 40×20,
80×24, 100×32 and 160×48, plus narrow-width tests with long fan names.

## Safety rules

- The TUI never writes NBFC or hardware directly; every action is an IPC request
  to the supervisor.
- Mode selection is immediate, but manual percentage changes require `y`/`n`.
- Without a reachable supervisor the interface is read-only and the footer says
  so.
- EC values are labelled EC, never RPM. Missing values stay `unavailable`.
- Blocked fans never receive a manual target or a closed-loop target.
- A failed recovery is surfaced on the Dashboard; it is never hidden.

## Tests

`cargo test --all-targets` covers navigation, direct mode application, manual
confirmation, Both-row editing, preset inference, profile apply/preview IPC
requests, active/failed profile rendering, help, footer messages and all
supported terminal sizes. No TUI test writes fan state.
