# NeoSense Audit — Index

> **Historical note (2026-11-08):** this audit describes the pre-Rust Bash implementation. The current Rust implementation and its resolved/deferred findings are documented in [`../IMPLEMENTATION.md`](../IMPLEMENTATION.md).

**Status:** `AUDIT COMPLETE — READ-ONLY`  
**Date:** 2026-11-08  
**Audited commit:** `ad2cd8b71e5d91542fe74ac22b27bdb7ddb5cc23` (`main`, tag `CLI-Release-NeoSense-v1.0.1`)  
**Repository:** `https://github.com/NeonMC23/NeoSense`

## Deliverables

| Document | Purpose |
|---|---|
| [`NEOSENSE_TECHNICAL_AUDIT.md`](NEOSENSE_TECHNICAL_AUDIT.md) | Full technical audit, sensor/safety analysis, CLI/TUI proposal, language comparison, target architecture, testing strategy and roadmap |
| [`EVIDENCE_LOG.md`](EVIDENCE_LOG.md) | Reproducible read-only commands and captured outputs supporting the findings |
| `README.md` | This index and audit-state statement |

## Headline findings

1. **Status dashboard is broken on the audited NBFC 0.5.3.** Case-sensitive AWK patterns never match NBFC's Title-Case keys; `fan_count` is empty; the dashboard prints `Current 0% / Target 0%` and a shell arithmetic error, then exits 0. The same human-output parser would fail on 0.5.4 unless the code switches to JSON.
2. **`neosense manual` cannot work.** It calls `nbfc set -f N` without `-s`/`-a`; NBFC requires one and exits 2.
3. **Mode detection never sees manual.** NBFC prints `false`, the code compares `"False"`.
4. **GPU fallback can report Wi-Fi/NVMe/ACPI temperature as GPU.** On the audit machine it selects `iwlwifi_1/temp1`.
5. **Fixed presets are unsafe open-loop.** `silent` is 25%, `set both 0` is accepted, with no sensor/temperature gate, confirmation, TTL or verification.
6. **`config use` is broken on NBFC 0.5.4 and unsafe everywhere.** `nbfc config -a` was removed; applying an unvalidated model config can damage hardware, per NBFC's own warning.
7. **Root is not a security boundary.** NBFC's socket is `0666`; fan control is allowed to any user; the root wrapper adds risk without protection.
8. **Read-back is not proof.** On the audit machine the EC values are outside the config's read range; NBFC clamps them and reports `Current 100%` regardless of target. This is not RPM.
9. **Both fans use `@CPU` sensors.** The GPU fan is driven by CPU temperature, not GPU temperature; NeoSense does not disclose this.
10. **Scripts are not executable in Git.** The documented `sudo ./install.sh` fails.

## Recommended direction

- **Do not start with a full rewrite.**
- **Phase 0:** fix correctness, parsing, installation, tests and documentation in Bash.
- **Phase 1:** add a read-only compiled sensor/diagnostic core with provenance, status and trust.
- **Phase 2:** add a dedicated `neosensed` safety supervisor; manual control only through it, with verification, TTL and fail-safe.
- **Phase 3:** coherent CLI with JSON and exit codes.
- **Phase 4:** cut over the core; keep Bash for install/packaging.
- **Phase 5:** TUI (Ratatui/Crossterm if Rust; Bubble Tea/Lip Gloss if Go).
- **Recommended language:** Rust for the core/daemon/TUI; Go is a credible alternative; Python is not recommended for the control core.
- **Minimum NBFC for protected write mode:** 0.5.4 (`status --json`, `reset-ec`, `ExecStopPost`). Read-only support can extend to 0.5.3.

## Git state and generated files

Initial state: clean `main` at `ad2cd8b71e5d91542fe74ac22b27bdb7ddb5cc23`, remote `https://github.com/NeonMC23/NeoSense`.

Generated (untracked documentation only):

```text
docs/audit/NEOSENSE_TECHNICAL_AUDIT.md
docs/audit/EVIDENCE_LOG.md
docs/audit/README.md
```

No production source, manifest, lockfile, systemd unit, system configuration or installed file was changed. No commit or push was made.

To verify:

```bash
git status --porcelain
git diff --stat
```

Expected: only untracked `docs/audit/` files; no modified tracked files.

## Audit method

- Cloned the official repository into an empty workspace.
- Read every source file and the README in full.
- Ran `bash -n` on all scripts.
- Ran ShellCheck 0.10.0 from a temporary tool directory (no system install).
- Exercised only read-only CLI paths (`--help`, `--version`, `status`, `config current/list`, `daemon status`, invalid speed/argument probes).
- Inspected local `hwmon`, thermal, NBFC status/config/state/socket and journal.
- Inspected upstream NBFC source at `880087d` and the 0.5.4 changelog to verify parsing, control, reset and JSON semantics.
- Used a safe invalid fan index (`nbfc set -f 999`) to test CLI behavior without changing any fan.

No fan speed was changed. No root operation was performed. No system package or configuration was modified.

## Remaining uncertainties

- Real fan RPM/read-back behavior across laptop models.
- Whether `ResetRequired=false` configs restore firmware control on service stop.
- GPU sensor mapping (`@GPU`/NVML) per NBFC config and driver.
- `nbfc reset-ec` behavior across 0.5.4 configs.
- Safety floor calibration per chassis.
- Suspend/resume behavior on 0.5.3 vs 0.5.4.
- Multi-fan/multi-GPU role mapping.
- Upstream NBFC socket permission changes.
- Long-term maintenance of Rust/Go TUI and NVML crates.

These require read-only diagnostics first, then supervised hardware-in-the-loop validation; they must not be guessed.
