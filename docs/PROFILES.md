# Thermal profiles

NeoSense has a deterministic, hardware-independent regulation engine. Profiles
can be previewed locally, simulated by the supervisor, or applied as a
supervisor-owned closed loop on fans that pass the safety policy.

## Modes

| Mode | What happens |
|---|---|
| Preview | Local target calculation from the current sensor snapshot. No supervisor request and no write. |
| Simulated | The supervisor selects the profile, evaluates periodically and reports targets. No backend write. |
| Applied | The supervisor regulates policy-eligible fans from fresh telemetry, verifies backend targets and returns blocked fans to automatic. |

A preview or simulated target is not an active fan setting. A successful backend
command is not measured RPM. EC percentages remain EC percentages, and physical
control validation stays separate from profile application.

## Reference curves

The curves below are reference policies. They can be applied to eligible fans by the supervisor; they are not universal hardware defaults.
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
regulation engine. They are usable in preview/simulation and can be applied by
the supervisor to policy-eligible fans; physical control validation remains
separate. See
[`CUSTOM_PROFILES.md`](CUSTOM_PROFILES.md) for the schema, CLI workflow,
atomic-write/backup behaviour and corruption recovery.

## Evaluation rules

- CPU fans require a fresh, valid, trusted CPU package reading.
- GPU fans require a fresh, valid, trusted GPU reading. They may use a GPU NBFC
  source, or an independent NVIDIA GPU die reading when NBFC declares a
  non-GPU source such as `@CPU`; the latter is an explicit warning and does not
  establish physical identity.
- Missing, stale or invalid telemetry produces `target_percent: null` and a
  reason; it never becomes 0% or a current reading.
- Targets are interpolated, clamped to the floor/ceiling, held inside the
  hysteresis deadband and limited by the maximum step.
- A fan with ambiguous role or no role-compatible temperature guard is unavailable.
- Evaluation is pure; it has no backend access and cannot write hardware.

## Current Acer result

Both NBFC fans declare `@CPU`. The CPU fan uses the trusted CPU package
reading. The GPU logical channel may use a fresh, trusted independent NVIDIA GPU
die reading as a guard, with an explicit warning and without changing NBFC.
Operator-attested CPU→LEFT and GPU→RIGHT actuator responses are recorded;
independent airflow/RPM identity remains unverified. Without the NVIDIA reading,
GPU regulation is blocked. EC readback is not RPM; provisional EC tach candidates
are never feedback. On the installed system, profiles apply to both logical
channels because both fans pass the current policy; a blocked fan would remain
automatic and be reported.

## Why some fans remain restricted

The host runs the reviewed NBFC 0.5.4 patch with a `0600 root:root` socket.
Control remains per-fan policy-gated. The following are still missing or
unverified for some or all fans:

- reliable independent fan identity and mapping;
- verified control range and semantics for each physical fan;
- validated RPM or other independent physical feedback (EC candidates remain provisional);
- reliable EC readback;
- physical fan identity and actuator-response validation;
- validated failure, shutdown and recovery behavior with real hardware;
- explicit physical-control validation.

The same rule applies to custom user curves: a persisted, schema-validated
curve is applied only to fans whose backend evidence and policy checks pass; blocked fans are returned to automatic.

Until all of these are demonstrated, `profile select` remains simulation only;
`profile apply` is policy-gated per fan and may be partial. Applying a profile
first returns every fan to automatic, so it takes ownership of unrelated
manual/preset fans; blocked fans are reported and left automatic. Per-fan
automatic removes one fan from the profile and keeps the remaining applied
fans regulated. Manual/preset/all-automatic takeover ends the profile. If a
write, readback or recovery check fails, the supervisor attempts verified
automatic recovery and reports a distinct failure when that recovery cannot be
confirmed. Physical control validation remains unproven.

## Commands

```bash
neosense profile preview quiet --json   # local preview, no supervisor
neosense profile preview desk --json    # preview a persistent user curve
neosense profile select quiet           # supervisor simulation, no write
neosense profile select desk            # select a user curve for simulation
neosense profile clear                  # deactivate active profile / clear simulation
neosense profile apply quiet --yes      # activate closed-loop for eligible fans
neosense profile apply desk --yes       # activate a persistent user curve
neosense profile apply quiet --dry-run  # validate without writing

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

The 2.3.0 release artifact remains frozen and unpublished. The policy engine
and supervisor-owned profile application are versioned as 2.4.0 development
because they add new public profile commands and per-fan control semantics.
A public release still requires the physical validation conditions above; the
development build is not evidence that those conditions are met.
