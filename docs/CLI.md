# NeoSense CLI reference

Command inventory, permissions and validation evidence are in
[`CLI_AUDIT.md`](CLI_AUDIT.md).

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

### `neosense sensors list|show <id>|watch [--interval N] [--once] [--json]`

Lists normalized sensors with role, value, trust, status, provider and path.
`watch` refreshes read-only until interrupted; `--once` prints one sample and
exits. With `--json`, each sample is one line of NDJSON so the stream remains
machine-readable. Missing values are `null` in JSON and `unavailable` in human
output.

### `neosense fan status [--json]`

Aligned fan table or JSON. Columns distinguish requested speed, backend target,
EC readback and RPM availability. EC is not RPM.

### `neosense diagnose [--json]`

Grouped report: summary, critical blockers, backend, hardware, thermal
monitoring, fan telemetry, control capabilities, security. JSON includes a
machine-readable `summary` and `checks[].group`.

### `neosense hardware list|current|identity`

Lists NBFC configurations or shows the selected one. `hardware validate <name>`
runs `nbfc rate-config` and requires root for ACPI access.

`hardware identity [--config NAME]` reads the selected (or named) NBFC
configuration JSON without modifying it. It reports logical fan names, roles,
read/write register pairs, sensor sources, algorithm, threshold count and
whether the readback range is usable. The report is explicitly read-only:
separate registers do not prove separate physical fans, and invalid
`MinSpeedValueRead`/`MaxSpeedValueRead` values mean NBFC `CurrentSpeed` is not a
trustworthy speed percentage.

### `neosense service status [--json]`

Distinct supervisor states:

| State | Exit |
|---|---:|
| healthy/connected | 0 |
| degraded | 6 |
| permission denied | 4 |
| not running/stale/protocol error | 7 |

### `neosense config show|path [--json]`

Shows compiled safety defaults/socket path. Safety settings are not
user-editable; persistent custom curves use the supervisor-owned profile store
described in [`CUSTOM_PROFILES.md`](CUSTOM_PROFILES.md).

### `neosense version [--json]`

Version and JSON schema identifier.

## Control commands

Control commands require the supervisor and pass the same policy as the TUI.

```bash
neosense fan auto
neosense fan set <cpu|gpu|name|index> <0-100> --yes [--dry-run]
neosense fan hold [fan] --yes [--dry-run]
neosense profile list|show <name>
neosense profile preview <name>
neosense profile select <name>
neosense profile state
neosense profile clear
neosense profile apply safe-auto --yes
neosense preset list
neosense preset show <name>
neosense preset apply <name> --yes [--dry-run]
```

- `fan auto` returns NBFC to automatic mode and verifies the result.
- `fan set`/`fan hold` are refused if sensors, mapping, backend guarantees or
  limits are not safe. Multi-fan `fan hold` resolves all targets before any
  write and auto-recovers on a later write failure; `--dry-run` never writes. `--dry-run` evaluates without writing but still needs a
  reachable supervisor because the policy is owned by `neosensed`.
- `profile preview` computes reference targets locally with no supervisor and no
  write.
- `profile select` selects a closed-loop profile in a 2.4.0 supervisor for
  simulation only; it never writes fan state. `profile state` shows the
  selected simulation targets, and `profile clear` removes the selection. A 2.3.0 supervisor returns `unknown IPC method` and
  must be upgraded first.
- `profile apply` currently accepts only `safe-auto`; closed-loop profiles are
  simulation/preview only.
- `preset apply` requires a 2.4.0 supervisor and applies the historical fixed presets through it.
  Presets are policy-gated and may be refused on this hardware; a successful
  backend target is not RPM evidence.
- `fan release` is explicitly unavailable until firmware/EC return is validated.
- `neosense tui --no-color` is rejected: the interactive TUI has a fixed
  semantic palette. Use CLI commands for colour-free output.
- `0%` is never accepted as a normal safe default.

## TUI and completions

```bash
neosense tui
neosense completions bash|zsh|fish|elvish|powershell
```

TUI actions and confirmation rules are documented in `docs/TUI.md`.

## JSON schemas

Object outputs carry a `schema` field where the command has a stable report:

| Command | Schema |
|---|---|
| `status --json` | `neosense.status/v1` |
| `sensors list/show/watch --json` | `neosense.sensors/v1`, `neosense.sensor/v1` |
| `fan status --json` | `neosense.fan_status/v1` |
| `profile show --json` | `neosense.profile/v1` |
| `profile preview --json` | `neosense.profile_preview/v1` |
| profile selection response | `neosense.profile_state/v1` |
| `preset list --json` | `neosense.preset_list/v1` |
| preset apply response | `neosense.preset_result/v1` |
| `profile custom list --json` | `neosense.custom_profiles_status/v1` |
| `profile custom show --json` | `neosense.custom_profile/v1` |
| custom mutation response | `neosense.custom_profile_result/v1` |
| `profile custom validate --json` | `neosense.custom_profile_validation/v1` |
| `profile custom path --json` | `neosense.custom_profile_path/v1` |
| `hardware list/current --json` | `neosense.hardware_list/v1`, `neosense.hardware_status/v1` |
| `hardware identity --json` | `neosense.hardware_identity/v1` |
| `service status --json` | `neosense.service_status/v2` |
| `diagnose --json` | `neosense.diagnose/v1` |
| `config show/path --json` | `neosense.config/v1`, `neosense.config_path/v1` |
| `version --json` | `neosense.version/v1` |

`profile list --json` is a stable array of evaluations. `hardware validate --json`
preserves the raw JSON emitted by `nbfc rate-config -j`; it is not wrapped by
NeoSense. `sensors watch --json` is NDJSON: one complete snapshot object per
line, not one large JSON array.

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
| 9 | Configuration/profile unavailable, invalid or corrupt |
| 10 | Verification failed |

## Safety

No command writes fan state directly. The supervisor is the only writer. If a
state cannot be verified, the command reports failure rather than a misleading
success.
