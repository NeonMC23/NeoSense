# NeoSense CLI reference

All commands support `--json` where applicable. `--json` writes valid JSON to
stdout and keeps diagnostics/errors out of the data stream. Read-only commands
do not require root; control uses the `neosensed` IPC socket.

## Global options

```text
--json               Machine-readable output
--no-color           Disable colour (NO_COLOR also honoured)
--socket <path>      Supervisor socket (env: NEOSENSE_SOCKET)
-h, --help
-V, --version
```

## Read commands

### `neosense status [--json]`

Shows hardware/NBFC summary, CPU/GPU temperatures, fan target/EC values,
sensor sources and shared warnings. Never writes.

### `neosense sensors list|show <id>|watch [--interval N] [--json]`

Lists normalized sensors with role, value, trust, status, provider and path.
`watch` refreshes read-only until interrupted. Missing values are `null` in
JSON and `unavailable` in human output.

### `neosense fan status [--json]`

Aligned fan table or JSON. Columns distinguish requested speed, backend target,
EC readback and RPM availability. EC is not RPM.

### `neosense diagnose [--json]`

Grouped report: summary, critical blockers, backend, hardware, thermal
monitoring, fan telemetry, control capabilities, security. JSON includes a
machine-readable `summary` and `checks[].group`.

### `neosense hardware list|current`

Lists NBFC configurations or shows the selected one. `hardware validate <name>`
runs `nbfc rate-config` and requires root for ACPI access.

### `neosense service status [--json]`

Distinct supervisor states:

| State | Exit |
|---|---:|
| healthy/connected | 0 |
| degraded | 6 |
| permission denied | 4 |
| not running/stale/protocol error | 7 |

### `neosense config show|path [--json]`

Shows compiled safety defaults/socket path. No user configuration file is
loaded in this version.

### `neosense version [--json]`

Version and JSON schema identifier.

## Control commands

Control commands require the supervisor and pass the same policy as the TUI.

```bash
neosense fan auto
neosense fan set <cpu|gpu|name|index> <0-100> --yes [--dry-run]
neosense fan hold [fan] --yes [--dry-run]
neosense profile list|show <name>|apply <name> --yes
```

- `fan auto` returns NBFC to automatic mode and verifies the result.
- `fan set`/`fan hold` are refused if sensors, mapping, backend guarantees or
  limits are not safe. `--dry-run` evaluates without writing.
- `profile apply` currently accepts only `safe-auto`; closed-loop profiles are
  preview-only.
- `fan release` is explicitly unavailable until firmware/EC return is validated.
- `0%` is never accepted as a normal safe default.

## TUI and completions

```bash
neosense tui
neosense completions bash|zsh|fish
```

TUI actions and confirmation rules are documented in `docs/TUI.md`.

## Exit codes

| Code | Meaning |
|---:|---|
| 0 | Success |
| 1 | Generic failure |
| 2 | Usage/invalid argument |
| 3 | Resource not found |
| 4 | Permission denied |
| 5 | Diagnostic failure |
| 6 | Degraded |
| 7 | Backend/service/supervisor unavailable |
| 8 | Unsafe operation refused |
| 9 | Configuration/profile unavailable/invalid |
| 10 | Verification failed |

## Safety

No command writes fan state directly. The supervisor is the only writer. If a
state cannot be verified, the command reports failure rather than a misleading
success.
