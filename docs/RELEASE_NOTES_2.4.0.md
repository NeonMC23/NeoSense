# NeoSense 2.4.0 — release notes

**Status:** release candidate; not publicly released. The 2.3.0 archive remains
frozen and unpublished. This version makes the terminal application the primary
fan-control interface and completes the everyday Auto / Silent / Balanced /
Maximum workflow.

## Highlights

### Everyday cooling

- **Auto** restores the NBFC temperature-dependent software curve, the normal
  automatic behavior on the validated Acer Nitro AN515-57.
- **Silent 25%**, **Balanced 60%** and **Maximum 100%** are fixed logical
  targets with supervisor preflight, backend verification, multi-fan leases and
  a 15-minute automatic recovery.
- **CPU / GPU / Both** manual control is bounded to 20–100%; 0% is refused.
- CLI preset commands accept `--yes` for explicit confirmation and `--dry-run`
  for policy validation without a write.

### Terminal UI

- Branded typographic identity: a bold ANSI Shadow-inspired `NEOSENSE` banner
  with the `LINUX THERMAL MANAGEMENT` descriptor and a telemetry rail on large
  terminals, a compact two-row wordmark at medium sizes and a one-line wordmark
  on small terminals. A strict printable-ASCII banner is selected automatically
  on non-UTF-8 locales or with `NEOSENSE_ASCII=1`.
- Consistent visual language across all seven views: compact header, filled
  active tab, inset rounded panels, one accent colour, and `●`/`▶` markers for
  active and selected states.
- Dashboard prioritises the active mode, backend targets, both fan states,
  temperatures, safety and warnings, with one-key mode selection.
- Fan Control adds a selectable **Both fans** row alongside CPU and GPU, a
  target bar, lease countdown and clearer requested/target/EC separation.
- Presets render as mode cards on wide terminals and an aligned list on narrow
  ones.
- Profiles show active, selected and blocked states distinctly, with live
  preview targets and curve detail.
- Mode selection (Auto, preset or profile) applies immediately. Manual
  percentage changes still ask `y`/`n`.
- Small-terminal layouts collapse gracefully; the tab bar switches to an index
  below 62 columns.

### Profiles

- Supervisor-owned closed-loop application with per-fan preflight, target
  verification, partial application and blocked-fan reporting.
- Preview (`profile preview`, TUI `p`) remains local or simulation-only and
  never writes fans.
- Persistent custom curves: versioned JSON, strict validation, atomic writes,
  backup, corruption recovery, CLI CRUD and TUI `$EDITOR` editing.
- Built-in curves are reference policies, not measured fan-response curves.

### Installation

- Unified `neosense.sh` entry point with distribution detection, DNF/APT/Pacman/
  Zypper dependency adapters, version checks, explicit privileged-operation
  authorization, NBFC/backend state detection, competing-controller reporting
  and read-only post-install verification.
- Missing or incompatible fan backends abort the default installation before
  software changes; `--software-only` is explicit and does not start the
  supervisor. NBFC configuration and unrelated controllers are never modified.
- Isolated installer-flow tests cover the supported distribution families,
  missing/old dependencies, backend states, conflicts, dry-run and
  non-interactive safety.

### Safety and operations

- `neosensed` remains the sole fan-write authority; the CLI and TUI use IPC only.
- Lease expiry, stale telemetry, lost access control, NBFC critical mode and
  failed verification trigger automatic recovery. An unconfirmed recovery is a
  distinct fail-safe state.
- Systemd `Type=notify`, `WatchdogSec=10s` and an `ExecStopPost` fail-safe.
- Installer supports in-place upgrade, creates a timestamped backup and restores
  the previous binaries/unit if verification fails. Uninstall preserves custom
  profiles and validation evidence.
- Host deployment of an earlier 2.4.0 candidate (`897e757`) was verified
  read-only: installed hashes matched, the supervisor reached READY, the
  watchdog advanced, the socket remained `root:neosense 0660`, and both fans
  returned to their normal automatic state. The final candidate, including the
  masthead-only TUI and the fan-message correction, has not been deployed to
  that host.
- `install.sh`/`uninstall.sh` expose `--help`, reject unsupported custom runtime
  and state paths for real systemd installs, and only print next-step hints that
  still apply. Release archives record package, target, profile, Rust and Cargo
  versions and are byte-for-byte reproducible.
- A genuine Fedora 44 KVM VM validation installed the release archive, started
  the supervisor without a backend, verified socket/permissions, exercised an
  upgrade, restored files after an injected unit failure, and preserved
  profile/validation data on uninstall. The official NBFC 0.5.4 Fedora RPM was
  checksum-verified and installed, but its service could not start without a
  laptop configuration or EC hardware. Backend-ready and physical validation
  remain separate.

## Validation

- CPU logical channel accepted at 0/50/100% (earlier supervisor session).
- GPU logical channel observed at 50/75/100% on 2026-11-17 with the CPU fan
  automatic, 80/75 °C ceilings and verified Auto recovery.
- Operator-attested mapping: CPU→LEFT, GPU→RIGHT.
- No independent RPM or airflow reference exists. Percentages are backend
  targets; EC readback is not RPM.
- See `docs/VALIDATION_RESULTS.md` for the authoritative record.

## Tests

452 automated Rust tests: 372 library, 71 CLI/integration, 7 low-level
installer, 1 English-only language guard, plus an additional binary test target.
A separate 54-case shell suite covers the unified installer's distribution
detection, dependency resolution, backend states, conflicts and dry-run safety. Coverage
includes policy, presets, profiles, custom-profile recovery, IPC, daemon
lifecycle, TUI navigation and rendering at small and large sizes, installer
rollback, PTY validation flows and JSON schemas.

## Known limitations

- No independent RPM/airflow calibration; physical identity of the EC channels
  remains unverified.
- EC readback is a clamp artifact with `IndependentReadMinMaxValues=false`.
- Both NBFC fans declare `@CPU`; the GPU logical channel relies on an
  independent NVIDIA temperature guard and is reported as a warning.
- Profiles are reference curves, not measured hardware response.
- `fan release` remains unavailable until EC/firmware return is validated.
- No broad laptop compatibility claim is made from a single machine.

## Upgrade notes

Run `sudo ./install.sh` from the new archive or source checkout. Existing
profiles and validation evidence under `/var/lib/neosense` are preserved. An
already-running supervisor is restarted and verified; its designed shutdown
fail-safe may return NBFC to automatic mode. Use `--no-restart` to defer the
restart.
