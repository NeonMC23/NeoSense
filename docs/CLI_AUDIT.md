# CLI audit and validation matrix

This document records the public command surface, its real behavior, required
privileges and the strongest evidence available. It distinguishes automated
tests from hardware validation.

## Common behavior

- `--json` writes one machine-readable object unless documented otherwise.
- Human diagnostics go to stderr; machine-readable results go to stdout.
- `--json` errors are emitted as `{"error":{"code", "message", "details"}}`
  with the same process exit code.
- Read commands never need root. State-changing commands use the
  `neosensed` Unix socket and the supervisor safety policy.
- `--socket` and `NEOSENSE_SOCKET` select the supervisor socket.
- `--no-color` and `NO_COLOR` disable ANSI colour; colour is never the only
  signal.

## Command inventory

| Command | Behavior | Privilege | Writes | JSON | Relevant tests |
|---|---|---|---|---|---|
| `status` | Hardware, NBFC, service, CPU/GPU, fans, warnings | none | no | object, `neosense.status/v1` | `status_json_is_valid_even_when_backend_is_missing`; host read-only run |
| `sensors list` | Typed sensors with provenance/trust | none | no | object, `neosense.sensors/v1` | `sensors_json_is_a_snapshot` |
| `sensors show <id>` | One sensor; exit 3 if absent | none | no | object, `neosense.sensor/v1` | `sensor_show_json_has_a_schema_when_sensors_exist`; missing-id path |
| `sensors watch` | Periodic read-only sampling | none | no | NDJSON snapshots | `sensors_watch_once_json...`, `sensors_watch_rejects_zero_interval` |
| `fan status` | Fan mode, target, EC, RPM/source | none | no | object, `neosense.fan_status/v1` | `fan_status_json_has_a_schema`; host read-only run |
| `fan auto` | Return to NBFC automatic mode | supervisor group | yes, via supervisor | control object | `control_without_supervisor_is_backend_unavailable`; container fake backend |
| `fan set` | Manual request, policy-gated | supervisor group | policy-gated | control object | `manual_without_confirmation_is_safety_refusal`; host refusal with exit 8 |
| `fan hold` | Hold target, policy-gated | supervisor group | policy-gated | control object | `hold_without_supervisor_never_writes` |
| `fan release` | Unavailable until EC/firmware recovery is validated | — | no | error | `release_is_explicitly_unavailable` |
| `profile list` | Profile catalogue/compatibility | none | no | array | `profile_catalogue_marks_curves_as_preview` |
| `profile show` | One profile evaluation | none | no | object, `neosense.profile/v1` | `profile_show_json_has_a_schema` |
| `profile apply` | Apply only `safe-auto` with `--yes` | supervisor group | `fan.auto` via supervisor | control object | `preview_profile_cannot_be_applied`; no-supervisor path |
| `hardware list` | NBFC config names | none | no | object, `neosense.hardware_list/v1` | host read-only run |
| `hardware current` | Selected NBFC config/status | none | no | object, `neosense.hardware_status/v1` | host read-only run |
| `hardware validate` | `nbfc rate-config -j` | root for some NBFC operations | no | raw NBFC JSON | not run on host in this session |
| `service status` | NBFC + supervisor state | none | no | object, `neosense.service_status/v2` | `service_status_distinguishes_*`, schema test |
| `diagnose` | Read-only categorized diagnostics | none | no | object, `neosense.diagnose/v1` | multiple CLI tests; host read-only run |
| `config show/path` | Compiled defaults/socket | none | no | object, `neosense.config*/v1` | `config_show_is_machine_readable` |
| `tui` | Interactive UI | none (read-only unless connected) | via supervisor only | n/a | render tests, pty smoke, rendered review |
| `completions` | bash/zsh/fish/elvish/powershell | none | no | n/a | `every_advertised_shell_completion_is_generated` |
| `version` | Version and schema | none | no | object, `neosense.version/v1` | `version_json_is_machine_readable` |

## Exit-code contract

| Code | Meaning | Representative command |
|---:|---|---|
| 0 | Success | read commands, confirmed control |
| 1 | Generic/internal failure | TUI startup failure |
| 2 | Usage/invalid argument/unsupported mode | `tui --json`, invalid percentage |
| 3 | Resource not found | unknown sensor/fan/profile |
| 4 | Permission denied | socket outside `neosense` group |
| 5 | Diagnostic failure | `diagnose` with a critical blocker |
| 6 | Degraded supervisor | `service status` |
| 7 | Backend/service/supervisor unavailable | control with no socket |
| 8 | Unsafe operation refused | manual set without `--yes`/policy refusal |
| 9 | Configuration/profile unavailable | preview profile apply, `fan release` |
| 10 | Verification failed | supervisor could not confirm a write |

## Known limitations

- `fan set --dry-run` still requires a reachable supervisor because the policy
  is evaluated by the supervisor; it never writes.
- `hardware validate` depends on NBFC and may require root; it is not exercised
  on this host by this session.
- Manual fan control remains refused on NBFC 0.5.3 and unvalidated hardware.
- Real systemd, root installation and group-session validation remain blocked
  by unavailable sudo authorization on the development host; isolated
  namespace/container validation is recorded separately in `docs/ROADMAP.md`.
