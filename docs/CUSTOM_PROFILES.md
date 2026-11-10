# Custom thermal profiles

Custom profiles are persistent **dynamic thermal curves**. They are not fixed
speed presets. A custom curve maps a trusted CPU or GPU temperature to a target
percentage and is evaluated continuously by the same regulation engine as the
built-in Quiet/Balanced/Performance curves.

Custom profiles are usable in preview and supervisor simulation even when no
hardware backend is validated. Selecting one never writes fan state, and the
current build does not apply any closed-loop curve to hardware.

## Two separate concepts

| Concept | Behaviour | Commands |
|---|---|---|
| Fixed preset | Requests fixed per-fan targets (Silent 25%, Balanced 60%, Performance 80%, Turbo 100%) | `neosense preset ...` |
| Dynamic profile | Calculates a target from temperature telemetry and a curve | `neosense profile ...` |
| Custom dynamic profile | A user-defined curve; same safety and capability gates | `neosense profile custom ...` |

A custom curve cannot change the semantics of a fixed preset, and a fixed preset
cannot silently become a curve.

## Where profiles are stored

The supervisor owns the persistent store:

```text
/var/lib/neosense/profiles.json
```

The systemd unit declares `StateDirectory=neosense`; the directory is owned by
`root:neosense` with mode `0750`. The supervisor is the only component that
writes it.

`NEOSENSE_PROFILES_PATH` overrides the path for both `neosense` and
`neosensed`. This is the supported mechanism for tests and isolated installs.

`neosense profile custom ...` uses supervisor IPC when a supervisor is
reachable. Without a supervisor it falls back to an unprivileged store:

```text
$XDG_CONFIG_HOME/neosense/profiles.json
# or
~/.config/neosense/profiles.json
```

The fallback exists so curves can be created, validated and previewed without a
service. A curve created only in the fallback is not visible to a running
supervisor until it is written through IPC. `neosense profile custom path`
prints the active path and scope.

## Document format

```json
{
  "schema": "neosense.custom_profiles",
  "version": 1,
  "profiles": [
    {
      "name": "desk",
      "title": "Desk curve",
      "description": "Quiet below 60 °C, stronger above 80 °C.",
      "points": [
        { "temperature_c": 40.0, "percent": 25.0 },
        { "temperature_c": 60.0, "percent": 45.0 },
        { "temperature_c": 80.0, "percent": 75.0 },
        { "temperature_c": 90.0, "percent": 100.0 }
      ],
      "min_percent": 20.0,
      "max_percent": 100.0,
      "hysteresis_c": 3.0,
      "max_step_percent": 10.0,
      "stale_after_ms": 3000,
      "fan_roles": ["cpu", "gpu"]
    }
  ]
}
```

### Profile fields

| Field | Rules |
|---|---|
| `name` | 1–64 characters; starts with a letter/digit; only `a-z`, `0-9`, `-`, `_`; unique; cannot be a built-in name |
| `title` | 1–80 characters, not blank |
| `description` | At most 500 characters; may be empty |
| `points` | 2–32 points; finite; strictly increasing temperature; 0–150 °C |
| `min_percent` | Finite; at least 20% |
| `max_percent` | Finite; at most 100%; greater than the minimum |
| point `percent` | Finite; between the profile minimum and maximum |
| `hysteresis_c` | 0–20 °C |
| `max_step_percent` | 1–50 percentage points |
| `stale_after_ms` | 1000–60000 ms |
| `fan_roles` | At least one of `cpu`, `gpu`; no duplicates; `unknown` is rejected |

Optional fields use documented defaults when omitted: `description` is empty,
`hysteresis_c` is 3.0, `max_step_percent` is 10.0, `stale_after_ms` is 3000 and
`fan_roles` is `["cpu", "gpu"]`.

Unknown fields, duplicate names, future versions and non-finite values are
rejected. The serialized store is limited to 32 KiB and 64 profiles so a full
supervisor status response remains below the IPC message limit; an oversized
store is rejected with an actionable error instead of breaking status/IPC.
Files larger than 256 KiB are rejected before JSON parsing.
Validation errors are returned verbatim so the CLI can show an actionable
message.

## CLI workflow

```bash
# Create from flags
neosense profile custom create \
  --name desk --title "Desk curve" \
  --point 40:25 --point 60:45 --point 80:75 --point 90:100 \
  --fans cpu,gpu

# Create from a complete JSON definition
neosense profile custom create --name desk2 --file desk2.json

# Inspect
neosense profile custom list
neosense profile custom show desk
neosense profile show desk
neosense profile custom path

# Update selected fields (unspecified fields are preserved)
neosense profile custom update desk --title "Desk v2" --point 40:30 --point 80:80

# Edit the whole JSON document in $VISUAL/$EDITOR
neosense profile custom edit desk

# Duplicate, rename, delete
neosense profile custom duplicate desk desk-copy
neosense profile custom rename desk-copy desk-winter
neosense profile custom delete desk-winter --yes

# Validate without writing
neosense profile custom validate --file candidate.json

# Preview and simulation
neosense profile preview desk
neosense profile select desk     # requires a 2.4.0 supervisor
neosense profile state
```

`profile preview` reads the current sensor snapshot and reports a calculated
target with a reason when a fan is unavailable. It does not contact the
supervisor or write hardware. `profile select` sends the selected name to the
supervisor, which evaluates it periodically; the response is explicitly
simulation-only.

## Validation and safety

Custom curves pass all existing checks:

- the normal curve validator (point count, ordering, bounds, hysteresis, rate
  limit, staleness);
- custom-profile policy limits (target floor, point range, fan roles);
- fan-role mapping (`@CPU` is not a GPU source);
- sensor freshness and trust;
- backend capability gates.

A custom profile does **not** bypass the supervisor. A selected custom curve is
still simulation-only. The supervisor will not apply it to hardware until the
backend has independently validated identity, range, feedback, recovery and
permissions.

## Atomic writes, backups and recovery

Every mutation:

1. takes an advisory `flock` on `profiles.json.lock`;
2. loads and validates the current document;
3. applies the mutation and re-validates the whole document;
4. copies the current main file to `profiles.json.bak`;
5. writes a mode-0600 temporary file in the same directory, fsyncs it and
   renames it over `profiles.json`;
6. fsyncs the directory.

The lock serialises concurrent CLI/supervisor updates. Temporary files are
removed on failure.

On load:

- a missing main file is an empty store; an existing backup is reported but is
  not loaded automatically (a deliberate deletion must not be undone silently);
- an invalid main file triggers recovery from `profiles.json.bak`;
- recovered data is reported as a warning, and the main file is repaired only
  on the next explicit mutation;
- if both files are invalid, the store reports `config_corrupt`, leaves both
  files untouched and refuses custom selection;
- a later mutation after an explicit main-file deletion replaces the stale
  backup instead of resurrecting old curves;
- built-in profiles continue to work.

`profile custom list` reports a corrupt store with exit code 9 and a diagnostic
object; it does not fabricate an empty profile list. Corruption is never
silently replaced with defaults. To recover explicitly:

```bash
neosense profile custom reset --yes
```

This preserves the old file as `profiles.json.corrupt-<timestamp>` and writes an
empty version-1 document. Without confirmation it refuses.

## Migration

- Version 1 (`schema: neosense.custom_profiles`, `version: 1`) is current.
- Version 0 is migrated in memory and rewritten as version 1 on the next save.
- A legacy bare JSON array of profiles is treated as version 0.
- A document with no version is rejected rather than guessed.
- A future version is rejected and left untouched.

## Diagnostics

`neosense diagnose` includes a `config.custom_profiles` check. When the
supervisor reports the store, the check shows the path and curve count, or a
failure/warning for corruption/recovery. Without a supervisor it reports the
unprivileged fallback store instead. This is read-only.

## Manual JSON editing

Manual edits to the supervisor store are validated on the next load. A valid
edit is accepted; an invalid main file falls back to the backup. Prefer the CLI
or the TUI editor so concurrent writes are serialised by the store lock.
