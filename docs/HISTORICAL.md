# Historical comparison: NeoSense v1.0.1 to 2.4.0 development

Reference tag: `CLI-Release-NeoSense-v1.0.1`, commit `ad2cd8b`.
The historical implementation is a Bash NBFC wrapper; it is a behavioural
reference, not an architecture to restore.

## Historical implementation

- `neosense`: command router/help.
- `src/commands.sh`: direct `nbfc set -a`, `nbfc set -f N -s PERCENT`, and
  preset functions.
- `src/config.sh`: NBFC config listing/applying.
- `src/status.sh`, `src/system.sh`, `src/utils.sh`: status, service and helpers.

The historical presets were:

| Preset | CPU | GPU |
|---|---:|---:|
| Silent | 25% | 25% |
| Balanced | 60% | 60% |
| Performance | 80% | 80% |
| Turbo | 100% | 100% |

The old wrapper called NBFC directly as root, identified CPU as fan index 0 and
GPU as index 1, accepted 0–100 without hardware mapping checks, and reported a
successful `nbfc set` as a successful fan change. It did not distinguish a
requested target from measured RPM.

## Modern mapping

| Historical feature | Modern status |
|---|---|
| Automatic mode | Preserved as supervised `fan auto` |
| Manual mode without changing speed | Replaced by supervised `fan hold` for current targets |
| CPU individual target | Preserved through supervisor `fan set cpu` policy |
| GPU individual target | Preserved but refused when mapping/permissions are unsafe |
| Both targets | Preserved through preset application or repeated supervised fan operations |
| Silent 25% | Implemented as fixed preset, policy-gated |
| Balanced 60% | Implemented as fixed preset, policy-gated |
| Performance 80% | Implemented as fixed preset, policy-gated |
| Turbo 100% | Implemented as fixed preset, policy-gated |
| NBFC config apply | Not restored as a control operation; config changes are outside the safety boundary |
| Daemon enable/disable | Replaced by systemd integration; service lifecycle remains explicit |
| Direct root NBFC writes | Replaced by supervisor IPC; UI never calls NBFC |

The old GPU index assumption is not treated as physical evidence. On the current
machine both NBFC fans use `@CPU`; the GPU target is blocked unless a real GPU
mapping is independently verified.

## Modern fixed presets

`presets.rs` contains the historical values. `preset apply` is sent to the
supervisor, which:

1. resolves fan roles and per-fan targets;
2. preflights every fan through the manual safety policy before writing;
3. applies targets only through the backend;
4. verifies accepted target and manual mode;
5. records a multi-fan lease;
6. requests automatic recovery on any failure.

On NBFC 0.5.3 or a mismatched mapping, the preflight returns a safety refusal
and no write occurs. This is intentionally stricter than v1.0.1. The preset
IPC method requires a 2.4.0 supervisor; the currently installed 2.3.0 daemon
returns `unknown IPC method` without a fan write.

## Modern dynamic profiles

Quiet/Balanced/Performance remain temperature-driven reference curves in
`regulation.rs`; they are selected for preview/simulation only. Their names
overlap historical fixed presets, so fixed presets are exposed through the
separate `preset` command and TUI Presets view.

Custom dynamic profiles remain unavailable until a persistent validated curve
workflow exists. They are not simulated as if configured.

## What was not restored

- Direct unprotected NBFC writes.
- Blind GPU fan index 1.
- Unchecked 0–100 writes without mapping/capability gates.
- NBFC configuration mutation from the application.
