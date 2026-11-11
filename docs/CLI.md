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
sensor sources, a separate RPM feedback section, control-readiness stage and
shared warnings. RPM channels use neutral names (`Acer Fan 1/2`) because
physical CPU/GPU identity is not verified. Never writes. Backend state **and
the sensor snapshot** come from the supervisor over IPC; the CLI never
connects to the NBFC socket and never reads sysfs/`nvidia-smi` itself. If the
supervisor is unavailable, the command says so explicitly instead of attempting
a direct NBFC connection. A group/world-writable NBFC socket produces a
distinct `NBFC_SOCKET_INSECURE` security warning; this is not a
hardware-capability warning.

### `neosense sensors list|show <id>|watch [--interval N] [--once] [--json]`

Lists the supervisor's normalized sensor snapshot with role, value, trust,
status, provider and path. The JSON `source` is `supervisor` when values came
over IPC and `unavailable` with a `source_error` when no snapshot is available;
the client never substitutes a local hardware read. Health explicitly carries `usable_for_regulation`, `usable_for_feedback` and
`physical_measurement`; the legacy `usable_for_control` equals regulation
eligibility. RPM sensors are physical feedback but never regulation inputs.
Acer RPM channels remain neutral (`Acer Fan 1/2`).
`watch` refreshes the supervisor snapshot read-only until interrupted; `--once`
prints one sample and exits. With `--json`, each sample is one line of NDJSON so
the stream remains machine-readable. Missing values are `null` in JSON and
`unavailable` in human output. A supervisor snapshot that ages after
serialization is marked not fresh when the client reports it.

### `neosense fan status [--json]`

Aligned fan table or JSON. Columns distinguish requested speed, backend target
and EC readback; EC is not RPM. A separate `rpm_feedback` array lists neutral
physical RPM channels with value, unit, trust, freshness,
`physical_measurement`, `usable_for_feedback` and
`usable_for_regulation=false`. Per-NBFC-fan RPM is not mapped because Acer
hwmon channels have no CPU/GPU labels. Backend state comes from the supervisor
over IPC, so an ordinary user does not need NBFC socket access.

### `neosense diagnose [--json]`

Grouped report: summary, critical blockers, backend, hardware, thermal
monitoring, fan telemetry, control capabilities, security. JSON includes a
machine-readable `summary` and `checks[].group`. The summary keeps distinct
states: `fan_control_available` (the protected manual path is policy-eligible,
not proof of physical control), `physical_feedback_available` (a real
RPM/tachometer channel exists), `physical_feedback_fresh`,
`physical_feedback_trusted` (valid, fresh, trusted physical feedback),
`physical_identity_verified` (CPU/GPU mapping established) and
`physical_control_validated`. It also includes a `control` object and the
summary fields `control_stage`, `control_mode` and `all_fans_eligible`. RPM is
feedback only and is never eligible for temperature regulation. A clean report
never implies physical control. `security.nbfc_socket` reports missing sockets,
non-socket paths, ownership, mode and group/world write exposure.
Manual/preset hardware gating requires the socket to be access-controlled.

### `neosense control readiness [--json]`

Returns `neosense.control_readiness/v1`: the stage (`unavailable`,
`hardware_detected`, `hardware_eligible`, `hardware_unvalidated`,
`hardware_validated`), runtime mode, selected profile mode, per-fan eligible and
blocked indexes, and one requirement per control/validation concern. Each
requirement carries a scope, status (`satisfied`, `missing`, `failed`,
`unverified`, `not_applicable`), detail and evidence. `hardware_validated`
requires explicit physical-identity, RPM-calibration and actuator-response
evidence; backend acceptance and RPM availability never satisfy it. The Acer
machine remains `hardware_detected` because the GPU fan still uses `@CPU`, and
its physical control remains unvalidated.

### `neosense hardware list|current|identity`

Lists NBFC configuration files by reading the configuration directories, or
shows the selected configuration from the supervisor. `hardware validate
<name>` asks the supervisor to run the read-only `nbfc rate-config` analysis;
the CLI never invokes NBFC itself. `hardware identity` is read-only and reports, per fan, the register pair, sensor sources, the
effective read range NBFC uses, its source (`independent`, `write_range` or
`invalid`) and whether physical identity/RPM are verified. A top-level
`feedback` object states the provenance explicitly: `rpm_source`
(`unavailable` or `hwmon`), `rpm_channels` (neutral names, value, trust,
freshness, `physical_measurement`, `usable_for_feedback`,
`usable_for_regulation=false`), `ec_readback` (`independent_scale`,
`uncalibrated_write_range` or `invalid`) and `physical_identity`
(`logical_only` or `physically_verified`). NeoSense never reports EC
percentages or an unused driver interface as RPM.

`hardware identity [--config NAME]` reads the selected (or named) NBFC
configuration JSON without modifying it. Sensor-source mapping is attached only
when the requested configuration is the one the supervisor is running; for any
other name the report deliberately leaves live mapping empty rather than
querying NBFC directly. It reports logical fan names, roles, read/write register
pairs, sensor sources, algorithm, threshold count and whether the readback range
is usable. The report is explicitly read-only:
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
Manual, preset and closed-loop hardware operations additionally require a
verified access-controlled NBFC socket; permission bits alone are not enough.
`0660 root:neosense` is rejected when ordinary IPC users are group members.
`fan auto` remains available for recovery.

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
| `hardware validate --json` | `neosense.hardware_validation/v1` |
| `service status --json` | `neosense.service_status/v2` |
| `diagnose --json` | `neosense.diagnose/v1` (includes `control`) |
| `control readiness --json` | `neosense.control_readiness/v1` |
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
