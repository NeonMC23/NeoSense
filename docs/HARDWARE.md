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
| OS / kernel | Nobara 44, 7.3.1-202.nobara.fc44.x86_64 at latest audit |
| NBFC | 0.5.3 at `/usr/bin/nbfc` (not an RPM package in this image) |
| NBFC config | `Acer Nitro AN515-57 Custom` |
| Fans | CPU fan and GPU fan |
| Fan register pairs | CPU read 19 / write 55; GPU read 21 / write 58 |
| Manual-mode setup | NBFC register writes 03=17, 34=12 (CPU), 33=48 (GPU) |
| Fan sensor mapping | both logical fans use `@CPU` |
| Readback range | `MinSpeedValueRead=0`, `MaxSpeedValueRead=0` for both fans — invalid |
| EC readback | 100.0% while targets were 65% and 70%; inconclusive and not a percentage |
| RPM | no hwmon tachometer; NBFC logs distinct raw EC values (CPU ~3448–3488, GPU ~4838–4918) but cannot map them |
| NBFC socket | `/run/nbfc_service.socket`, mode `0666`, `root:root` |
| Power policy | `power-profiles-daemon` active; other checked controllers inactive |
| NeoSense supervisor | installed 2.3.0 supervisor active and healthy under `sg neosense`; 2.4.0 development build not installed |

The latest read-only audit found no hwmon PWM or fan-input channel, no
`fan*_input` tachometer and no `/sys` PWM interface. `acer_wmi` is loaded but
exposes no fan hwmon interface. `nvidia-smi` reports the RTX 3050 fan speed as
`[N/A]`. The GPU temperature is exposed by `nvidia-smi` and is displayed by
NeoSense, but the current NBFC configuration does not use it for the GPU fan.

`neosense hardware identity` now reads the configuration document directly. It
shows two distinct logical register pairs (`19/55` and `21/58`) and separate
manual-mode initialization writes, so the NBFC configuration distinguishes two
logical channels. This is **not** proof of two independently controllable
physical fans: both channels use `@CPU`, and the readback range is invalid
(`MinSpeedValueRead == MaxSpeedValueRead == 0`). The physical mapping and
airflow independence therefore remain unverified.

The task brief described an i7-11800H/RTX 3060; the actual audited machine is
an i5-11400H/RTX 3050. Compatibility claims follow the observed machine, not
the brief.

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

## NBFC 0.5.4 upstream source evidence

The 0.5.4 tag was inspected in a temporary clone (not installed on the host).
It adds:

- `nbfc status --json`, served by `server.c` with `PID`, `SelectedConfigId`,
  `ReadOnly`, and `Fans[]` fields `Name`, `Temperature`,
  `CriticalTemperature`, `AutoMode`, `Critical`, `CurrentSpeed`,
  `TargetSpeed`, `RequestedSpeed`, `SpeedSteps`;
- `nbfc reset-ec`, which resets register-write configurations and fans and is
  used as `ExecStopPost` in NBFC's own systemd unit;
- `nbfc test-config run|evaluate` for temperature/fan-speed correlation;
- per-fan `CriticalTemperature` overriding the model value;
- hardware-change detection and higher hwmon index limits;
- removal of `nbfc config -a` in favour of `config -s`.

**Security did not improve:** 0.5.4 still hardcodes `chmod 0666` in
`src/server.c` and has no supported socket-permission option. NeoSense's JSON
parser already accepts the new status shape, but the JSON contract alone does
not justify manual control on this hardware.

These are necessary capabilities, not proof that this laptop is safe for
manual control. Installing NBFC 0.5.4, changing its configuration, or testing
fans requires explicit authorization and a validated recovery plan. A future
adapter could use `reset-ec` and `test-config`, but only after mapping, RPM or
independent feedback, and recovery are validated.

## Upstream access-control audit

The NBFC Linux source was inspected at tags `0.5.3`, `0.5.4` and current
master commit `880087d`:

- every inspected `src/server.c` contains `chmod(NBFC_SOCKET_PATH, 0666)`
  immediately after `bind()`;
- the socket path is a compile-time `RUNSTATEDIR "/nbfc_service.socket"`;
  the shipped systemd unit has no socket-permission or socket-activation
  setting;
- the README capability table states **Fan Control Rights: Any user**;
- `0.5.4` adds JSON status, `reset-ec`, `test-config`, per-fan critical
  temperatures and hardware-change detection, but does not add access control;
- current master adds root-only suspend/resume checks using `SO_PEERCRED`, but
  fan commands remain unrestricted.

There is therefore no supported NBFC option that makes the socket exclusive to
NeoSense. A robust fix requires an upstream change (configurable ownership/mode
or peer-credential authorization on fan commands) or another verified
access-control mechanism. A periodic `chmod` or socket-deletion race is not
acceptable.

NeoSense refines the local check further: `0660 root:neosense` is **not**
accepted merely because it lacks world-write. The `neosense` group contains an
ordinary local CLI user, so group write is an access path. A dedicated group is
accepted only when local membership is fully resolved and contains no
non-supervisor user; POSIX ACLs, owner mismatches and unverified supervisor
identities fail closed.

## Alternative backend assessment

No supported alternative on this machine currently exposes independent fan
channels with trustworthy feedback. The kernel exposes no PWM/fan hwmon path;
`acer_wmi` does not provide a documented fan-control sysfs interface here; and
NVIDIA tooling exposes no fan RPM. NBFC remains the only observed fan backend,
and its direct socket bypass remains an unresolved ownership problem. Adding a
second daemon or writing EC registers would violate the safety boundary.

## Backend capability matrix

| Backend | Detection | Fan identity | Fixed targets | Target readback | RPM | Permission model | Status on this host |
|---|---|---|---|---|---|---|---|
| NBFC 0.5.3 | `nbfc --version`, text `status` | Two logical names with distinct read/write registers; both use `@CPU` | Per-index `set -f` exists but is refused by policy | `CurrentSpeed` only; readback range invalid, so not a trustworthy percentage | None | NBFC socket `0666`; NeoSense socket `0660` | Observed; manual control blocked |
| NBFC 0.5.4+ | version + `status --json` | Same logical names; mapping still needs evidence | Per-index request; protected manual gate | JSON `TargetSpeed` / `CurrentSpeed` (EC percentage, not RPM) | None | Upstream socket still `0666` | Not installed; would still need mapping/feedback validation |
| hwmon PWM | `pwm*`, `fan*_input` under `/sys/class/hwmon` | Per hwmon channel | Possible if a driver exposes it | PWM value | `fan*_input` when present | sysfs ownership | No PWM/fan input observed |
| `acer_wmi` | loaded kernel module | Not exposed as a documented fan channel | Not exposed | Not exposed | Not exposed | kernel | Loaded; no usable interface found |
| `nvidia-smi` | NVIDIA driver | GPU telemetry only | Not a fan-control API | Temperature/power; fan `[N/A]` | Not available | user read | Telemetry only; never a fan channel |

This matrix records observed or documented capabilities, not a promise that a
backend is safe to enable. NeoSense does not add a backend merely to increase
coverage.

## Capability matrix for this machine

| Capability | Evidence | Status | Remaining requirement |
|---|---|---|---|
| Fan identity | NBFC config exposes CPU 19/55 and GPU 21/58; NBFC logs distinct raw values for the two channels | Logical identity only | Independent physical fan/airflow evidence |
| Per-fan target | NBFC `set -f` with separate write registers exists | Upstream/config-supported; unvalidated | NBFC >= 0.5.4, access control, explicit authorization |
| Target acceptance | NBFC status `TargetSpeed`; no protected manual contract on 0.5.3 | Read-only observed only | Protected status/manual semantics and a verified write |
| Backend readback | `CurrentSpeed` present but `MinSpeedValueRead=MaxSpeedValueRead=0` | Invalid | Repair/validate register mapping or use another feedback source |
| Physical RPM feedback | No hwmon `fan*_input`; NVIDIA fan `[N/A]`; raw NBFC EC values are uncalibrated | Unavailable | Tachometer or calibrated read-only EC measurement |
| CPU temperature mapping | coretemp/x86_pkg_temp; both fans use `@CPU` | Read-only validated | Confirm fan-to-sensor relationship |
| GPU temperature mapping | NVIDIA GPU temperature available; GPU fan still uses `@CPU` | Unavailable for fan mapping | Device-specific mapping change and validation |
| Automatic-mode recovery | NBFC auto state; supervisor `fan auto` verified in installation lifecycle | Read-only/observed | Recovery behavior under backend failure/crash |
| Access control | NBFC socket `0666 root:root`; upstream hardcoded; `0660 root:neosense` would also expose ordinary IPC users | **Unsuitable** | Upstream fix or verified restrictive access-control mechanism |

The only hardware operation performed in previous installation cycles was the
authorized `nbfc set -a` automatic-mode fail-safe during supervisor restarts.
No manual target was sent.

### Raw readback clue

NBFC periodically logs raw values such as CPU `3448–3488` and GPU
`4838–4918` with “not range of MinSpeedValueRead/MaxSpeedValueRead”. These
values change over time and differ by channel, which is consistent with raw
tachometer-like counts, but:

- the config declares `MinSpeedValueRead = MaxSpeedValueRead = 0`;
- no unit, scale or calibration is known;
- no independent tachometer or physical observation confirms them;
- NBFC reports `CurrentSpeed` as an invalid percentage as a result.

They must not be reported as RPM. Establishing whether they are tach counts
requires a controlled read-only capture plus a known physical reference, or an
external tachometer. Any controlled fan-speed change requires explicit
authorization and a recovery plan.

## Future backend acceptance criteria

A future backend should provide stable fan identity, per-fan sensor mapping,
explicit mode/state transitions, raw tachometer or another independent physical
feedback signal, critical-temperature enforcement, EC reset/recovery, and a
way to detect hardware changes. Until then, NeoSense remains read-only for
manual control and reports EC percentages only as EC percentages.

Persistent custom curves do not change this boundary. They are validated and
evaluated by the regulation engine, but selection is simulation-only and cannot
reach the backend until the same capability, mapping, feedback and recovery
evidence exists.
