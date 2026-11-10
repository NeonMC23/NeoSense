# Thermal profile engine and simulation boundary

NeoSense now has a deterministic, hardware-independent regulation engine for
profile previews and supervisor simulation. It does **not** apply these targets
to fan hardware on this machine.

## Modes

| Mode | What happens |
|---|---|
| Preview | CLI computes targets from the current sensor snapshot. No supervisor request and no write. |
| Simulated | Supervisor selects the profile, evaluates periodically and reports targets. No backend write. |
| Hardware | Not enabled. Requires the validation conditions below. |

A simulated target is not an active fan setting. A successful backend command
is not measured RPM. EC percentages remain EC percentages.

## Reference curves

The curves below are reference simulation policies, not hardware defaults.
They are monotonic, bounded, and intentionally simple. They were not derived
from a measured fan-response curve on the Acer Nitro hardware.

| Profile | Points (°C → %) | Floor | Hysteresis | Max step |
|---|---|---:|---:|---:|
| Quiet | 40→20, 55→30, 70→50, 82→75, 90→100 | 20% | 3 °C | 10% |
| Balanced | 40→25, 55→40, 70→65, 82→90, 90→100 | 20% | 3 °C | 10% |
| Performance | 40→35, 55→55, 70→80, 82→100, 90→100 | 20% | 3 °C | 10% |
| Safe Auto | no curve; NBFC automatic mode | — | — | — |
| Custom | user-defined persistent curve; see [`CUSTOM_PROFILES.md`](CUSTOM_PROFILES.md) | configurable, ≥20% | configurable | configurable |

Built-in curve validation requires at least two points, finite values, strictly
increasing temperatures, 0–100 percentages, a valid floor/ceiling,
non-negative hysteresis, a positive rate limit and a positive staleness window.

Custom profiles are dynamic curves, not fixed presets. They are stored in a
versioned JSON document, validated with additional policy limits (floor ≥20%,
2–32 points, 0–150 °C, fan-role selection) and evaluated through the same pure
regulation engine. They are usable in preview and supervisor simulation while
hardware control remains unavailable. See
[`CUSTOM_PROFILES.md`](CUSTOM_PROFILES.md) for the schema, CLI workflow,
atomic-write/backup behaviour and corruption recovery.

## Evaluation rules

- CPU fans require a fresh, valid, trusted CPU package reading.
- GPU fans require a fresh, valid GPU reading and a sensor mapping that
  actually mentions a GPU source.
- Missing, stale or invalid telemetry produces `target_percent: null` and a
  reason; it never becomes 0% or a current reading.
- Targets are interpolated, clamped to the floor/ceiling, held inside the
  hysteresis deadband and limited by the maximum step.
- A fan with ambiguous identity or mismatched sensor mapping is unavailable.
- Evaluation is pure; it has no backend access and cannot write hardware.

## Current Acer result

Both NBFC fans use `@CPU`; the GPU fan mapping therefore fails the GPU source
check. The CPU fan can produce a simulated target from a trusted CPU reading.
The GPU fan is explicitly unavailable. EC readback is not RPM and is not used
as physical feedback.

## Why hardware control remains disabled

The following are still missing or unverified on this machine:

- reliable independent fan identity and mapping;
- verified control range and semantics for each physical fan;
- reliable RPM or other independent physical feedback;
- reliable EC readback;
- validated backend ownership and permission model (NBFC's own socket is
  `0666`; direct bypass remains possible);
- validated failure, shutdown and recovery behavior with real hardware;
- explicit hardware authorization.

The same rule applies to custom user curves: a persisted, schema-validated
curve is still simulation-only until the backend evidence above exists.

Until all of these are demonstrated, `profile select` is simulation only,
`profile apply` remains limited to `safe-auto`, and manual fan control remains
blocked.

## Commands

```bash
neosense profile preview quiet --json   # local preview, no supervisor
neosense profile preview desk --json    # preview a persistent user curve
neosense profile select quiet           # supervisor simulation, no write
neosense profile select desk            # select a user curve for simulation
neosense profile clear                  # clear simulation selection
neosense profile apply safe-auto --yes  # authorized NBFC automatic mode

neosense profile custom list
neosense profile custom create --name desk --point 40:25 --point 80:75
neosense profile custom edit desk
neosense profile custom delete desk --yes
```

`profile preview` exits with a clear error if NBFC status is unavailable.
`profile select`/`clear` require a connected **2.4.0** supervisor; a 2.3.0
supervisor returns `unknown IPC method` (no fan write). Neither command is
hardware validation.

## Versioning decision

The 2.3.0 release artifact remains frozen and unpublished. This simulation
engine is versioned as 2.4.0 development because it adds new public profile
commands and a policy engine. A release that applies closed-loop targets to
real hardware requires a later version and all validation conditions above.
