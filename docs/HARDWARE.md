# Hardware and backend evidence

This page records read-only evidence for the development machine and the
criteria a future backend must satisfy before NeoSense enables manual fan
control. It is not a compatibility guarantee.

## Current machine

| Item | Observed value |
|---|---|
| Model | Acer Nitro AN515-57 |
| CPU | Intel Core i5-11400H |
| GPU | Intel TigerLake-H UHD + NVIDIA GA107M GeForce RTX 3050 Mobile |
| OS / kernel | Nobara 44, 7.3.0-203.nobara.fc44.x86_64 |
| NBFC | 0.5.3 at `/usr/bin/nbfc` (not an RPM package in this image) |
| NBFC config | `Acer Nitro AN515-57 Custom` |
| Fans | CPU fan and GPU fan |
| Fan sensor mapping | both fans use `@CPU` |
| EC readback | 100.0% while targets were 35% and 40%; inconclusive |
| RPM | no readable tachometer in hwmon |
| NBFC socket | `/run/nbfc_service.socket`, mode `0666`, `root:root` |
| Power policy | `power-profiles-daemon` active; other checked controllers inactive |
| NeoSense supervisor | inactive; requires root/systemd |

The GPU temperature is exposed by `nvidia-smi` and is displayed by NeoSense,
but the current NBFC configuration does not use it for the GPU fan. The GPU fan
therefore cannot be justified as independently controllable.

## Why manual control is blocked

NeoSense requires all of the following before a manual write:

1. fresh, trusted CPU and GPU evidence when a GPU fan is targeted;
2. a known fan-to-sensor mapping that matches the requested role;
3. a backend that exposes protected status/readback and recovery guarantees;
4. a verified floor, ceiling and critical-temperature policy;
5. readback that can be checked after the write, without treating EC percentage
   as RPM;
6. a tested automatic/firmware recovery path.

NBFC 0.5.3 provides neither the JSON status contract nor `reset-ec`, and the
current EC readback cannot confirm that a requested target was reached. The
policy therefore refuses `fan set`/`fan hold` on this machine. `fan auto` is
the only exposed control request, and it still needs an authorized supervisor.

## NBFC 0.5.4 upstream evidence

The upstream release page lists NBFC 0.5.4 (2026-10-10) with:

- `nbfc status --json`;
- `nbfc reset-ec` for the systemd recovery path;
- `nbfc test-config` for temperature/fan-speed correlation;
- per-fan `CriticalTemperature`;
- hardware-change detection.

These are necessary capabilities, not proof that this laptop is safe for
manual control. Installing NBFC 0.5.4, changing its configuration, or testing
fans requires explicit authorization and a validated recovery plan.

## Future backend acceptance criteria

A future backend should provide stable fan identity, per-fan sensor mapping,
explicit mode/state transitions, raw tachometer or another independent physical
feedback signal, critical-temperature enforcement, EC reset/recovery, and a
way to detect hardware changes. Until then, NeoSense remains read-only for
manual control and reports EC percentages only as EC percentages.
