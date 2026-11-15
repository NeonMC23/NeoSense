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
| OS / kernel | Nobara 44, 7.3.4-200.nobara.fc44.x86_64 at latest audit |
| NBFC | patched 0.5.4 at `/usr/bin/nbfc` (not an RPM package in this image) |
| NBFC config | `Acer Nitro AN515-57 Custom` |
| Fans | CPU fan and GPU fan |
| Fan register pairs | CPU read 19 / write 55; GPU read 21 / write 58 |
| Manual-mode setup | NBFC register writes 03=17, 34=12 (CPU), 33=48 (GPU) |
| Fan sensor mapping | both logical fans use `@CPU` |
| Readback range | `IndependentReadMinMaxValues=false`; effective range is the write range `0..100`; the declared read values `0/0` are ignored |
| EC readback | `CurrentSpeed=100%` from clamping raw 16-bit reads (CPU ~4225–4615, GPU ~5769–6122); not a measurement |
| RPM | No Acer hwmon fan input on this kernel. Provisional read-only EC candidates at `0x13/0x14` and `0x15/0x16` observed as `3896`/`5263`; not validated, not mapped, never feedback |
| NBFC socket | `/run/nbfc_service.socket`, mode `0600`, `root:root`; `/etc/nbfc/access.json` root-only |
| Power policy | `power-profiles-daemon` active; other checked controllers inactive |
| NeoSense supervisor | `fa4e7ff` release installed 2026-11-15; active and healthy; CLI/TUI read backend state and sensor snapshots over IPC only |

The upstream `Acer Nitro AN515-57.json` configuration also declares
`MinSpeedValueRead = MaxSpeedValueRead = 0` with
`IndependentReadMinMaxValues=false`, and uses `@CPU` for both logical fans. The
installed `Acer Nitro AN515-57 Custom.json` is a threshold/EcPoll customization
of that upstream file. The omitted independent read scale is inherited from
upstream; the sibling-model audit below shows that this is an AN515-57/AN517-52
configuration outlier, not a property of the register family. Neither file
establishes independent GPU fan control or usable RPM feedback.

On kernel `7.3.4-200`, stock `acer_wmi` (`3B2CBCF7…`) is loaded, but the
AN515-57 is not enabled as a Predator V4 hwmon device; no Acer hwmon fan input
exists. The module exposes `predator_v4=N` and `ec_raw_mode=N`; changing either
would require a module reload and is outside this campaign. There is no `/sys`
PWM interface and no `platform_profile`. `nvidia-smi` reports GPU temperature
but fan speed `[N/A]`.

NBFC's `nvidia-ml` provider reports `none` and both logical fans still declare
`@CPU`. NeoSense therefore does not use NBFC's source as a GPU thermal
substitute. Instead, when a fresh, trusted NVIDIA GPU die reading is available,
the GPU logical channel may use that independent guard, with an explicit
warning and no claim of physical fan identity. A missing, stale or untrusted
NVIDIA reading closes GPU eligibility immediately. NBFC is not reconfigured.

### ACPI fan objects (read-only)

The firmware does expose generic ACPI fan devices:
`/sys/class/thermal/cooling_device0..3` are type `Fan`, backed by
`PNP0C0B:00..03` (a fifth `PNP0C0B:04` exists without a cooling-device
binding). Every one reports `max_state=1` and `cur_state=0`: the firmware
exposes only on/off fan states, not a proportional PWM channel and not a
tachometer. They are therefore not usable as RPM feedback or as a proportional
control interface, and NeoSense does not write them.

Inside a single one-second window the two NBFC channels report different raw
values (for example CPU `4285` / GPU `5660`, then CPU `4225` / GPU `5769`),
which supports two distinct read channels; it does not identify which physical
fan each channel drives.

### ACPI/AML analysis (read-only, 2026-11-11)

The DSDT and SSDTs were copied through a privileged read and inspected
offline; no ACPI method was executed and no EC/ACPI write was performed.

- The EC device `EC0_` (`PNP0C09`) defines the `ECMM` operation region. Its
  field list contains `FAN1`, `FAN2` and `FANT` as **1-bit** fields at EC byte
  ~171 (`0xAB`), alongside `MODE` and `INIT`. These are status/control flags,
  not 16-bit tach values, and they are unrelated to NBFC's read registers 19
  (`0x13`) and 21 (`0x15`).
- The DSDT also defines `FANG`/`FANW` methods that call `EIDR`/`EIDW`; they
  were not called.
- SSDT10 (`ACRSYS`/`ACRPRDCT`, Acer) defines firmware sensor descriptor
  packages. The descriptors include:
  - **“CPU Fan Duty Cycle”**, unit `RAW`;
  - **“CPU Fan #1 Speed”**, unit **`RPM`**;
  - skin/thermistor temperatures, platform/battery power and capacity.
  It also defines `RPMD`, `OSDL`, `PADD/PADT`, `MTMP` and `WPMD` methods. This
  is the firmware's own fan telemetry model and is strong evidence that the
  platform has an RPM-scale fan value, but it names only CPU fan #1, not a GPU
  fan.
- The Linux WMI bus exposes 17 ACPI-WMI GUID devices. `acer_wmi` is loaded
  with `predator_v4=N`; on kernel 7.3.4-200 it exposes no Acer hwmon fan input
  (`hwmon` has no `name=acer` entry). The earlier 7.3.1-202 hwmon observation
  is historical. `/proc/acpi/call` is absent, `acpi_call` is not loaded, and
  `acpica-tools` is not installed.
- The firmware RPM descriptor remains a candidate only. Current read-only EC
  bytes are exposed by NeoSense as **provisional, unmapped** tachometer
  estimates. They are not validated feedback and cannot drive regulation.
  Do not enable `predator_v4`, use `ec_raw_mode`, or load `acpi_call` without
  separate authorization.

### acer_wmi driver compatibility (historical source audit before candidate deployment)

Inspected implementation: kernel `7.3.1-202.nobara.fc44.x86_64`,
`/lib/modules/.../acer-wmi.ko.zst` (srcversion
`3B2CBCF7ACAF98B534919E6`), and upstream
`drivers/platform/x86/acer-wmi.c` (GPL-2.0-or-later). The Nobara kernel-devel
tree does not ship `acer-wmi.c`. The local module is slightly older than
current upstream master: it lacks the upstream `AN16-41`, `ANV15-51` and
`A315-58G` DMI entries. It does contain the `AN515-58` entry and the gaming/
HWMON symbols, so the traced read/write design applies; the exact Nobara
source tarball was not available.

**Model matching.** `find_quirks()` uses a DMI table unless `predator_v4=1` is
set. Relevant entries:

- `Acer Nitro AN515-58` and `Nitro AN16-41` →
  `hwmon=true, platform_profile=true, pwm=true`;
- `Predator PH315-53` → turbo/fan counts, no hwmon;
- Predator V4 models → `hwmon=true, platform_profile=true`, no `pwm`;
- **AN515-57 and AN517-52 are absent**; the blacklist only covers
  AOA110/AOA150.

This is a missing DMI quirk / untested compatibility gap, not a deliberate
blacklist.

**Firmware interface.** The gaming path uses `WMID_GUID4`
`7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56`. That GUID is present in SSDT12
(`_WDG`) and as a WMI bus device on this machine, so the AN515-57 firmware
does expose the Acer gaming WMI GUID. Whether the specific method IDs
(5, 14–17, 22–23) and sensor bitmask are implemented is unproven; that would
require an actual call, which this task did not make.

**Read path (no writes).** `acer_wmi_hwmon_read()`:

- fan speed: `GET_GAMING_SYS_INFO` (`GET_PREDATOR_V4_SENSOR_READING`, sensor
  IDs CPU `0x02`, GPU `0x06`), returned unscaled as an hwmon fan input;
- temperature: the same GET command with temperature sensor IDs;
- PWM read: `GET_GAMING_FAN_SPEED`/`GET_GAMING_FAN_BEHAVIOR` (queries only).

`acer_wmi_hwmon_init()` performs one `GET_SUPPORTED_SENSORS` query and
registers hwmon. Probe, registration and read paths issue no SET command.

**Write path.** `hwmon_write` exists only for PWM, and `is_visible` hides PWM
attributes unless `ACER_CAP_PWM` is set (`quirk->pwm`). The `predator_v4`
quirk does **not** set `pwm`, so forcing it would create fan inputs at `0444`
and no `pwm*` files. However, it also registers `platform_profile`, whose
`set` writes an Acer thermal profile (`SET_GAMING_MISC_SETTING`) and can
change firmware fan behavior; the mode-key handler
(`cycle_gaming_thermal_profile=Y`) can do the same. A probe-time
`GET_SUPPORTED_SENSORS` failure makes the platform probe fail and tears down
rfkill/backlight.

**Historical read-only RPM telemetry (kernel 7.3.1-202, 2026-11-11).** The audited candidate was
installed and active at that time: `acer_wmi` srcversion
`99563E2DB1E30D449538A29`, decompressed module SHA-256
`41b30ad16c1c8b126c430c0cd2a1c689edfb2dfea8ca8222af38a2fdd6084fa4` (compressed
`39c32554…`). The kernel exposes `hwmon6`, `name=acer`, with
`fan1_input`/`fan2_input` at `0444`. There are no fan labels, no `pwm*`
attributes and no `/sys/firmware/acpi/platform_profile`. The read-only Acer
`temp1..3` channels are also present; they are role-unknown/untrusted and are
not regulation inputs.

Those historical samples observed Acer Fan 1 at `3488–4918 rpm` and Acer Fan 2
at `4918–6382 rpm`. They are not available on the current kernel. Current
read-only EC bytes at `0x13/0x14` and `0x15/0x16` produced `3896` and `5263` in
the campaign audit, but NeoSense reports them as `Trust::Provisional` with
`usable_for_feedback=false` and no logical-fan mapping. They do not establish
calibration, physical mapping or fan-control validation.

A read-only EC snapshot during the campaign also showed `0x21=0x30`,
`0x22=0x0c`, `0x37=0x2d`, and `0x3a=0x32`. The community `acer-nitro-ec`
mapping interprets `0x21/0x22` as GPU/CPU manual-mode selectors and
`0x37/0x3a` as 45%/50% targets, while NBFC reported both fans as automatic.
This discrepancy is unresolved: it may be a stale/unused mode register, a
different firmware path, or a real ownership mismatch. No register was
written; physical validation must resolve it before any target test.

`neosense hardware identity` reads the configuration document directly. It
shows two distinct logical register pairs (`19/55` and `21/58`) and separate
manual-mode initialization writes, so the NBFC configuration distinguishes two
logical channels. Its `feedback` object reports no validated RPM source; the
provisional EC tach candidates are not attached to logical fans and are never
used for regulation. The two channels also report different raw values in the
service journal. Combined with the register-family evidence below, this raises
confidence that the platform exposes two independent tach-like channels. It is
still **not** proof of two independently controllable physical fans: both
sensors use `@CPU`, and no external observation links a channel to an airflow
path. Physical identity remains “supported by family convention, unverified”.

### Acer register-family evidence

The 323 JSON configurations shipped with NBFC 0.5.4
(`share/nbfc/configs`) were audited read-only. `read=19 write=55` and
`read=21 write=58` are a consistent Acer Nitro/Predator family convention:

| Model | CPU channel | GPU channel | Independent read scale |
|---|---|---|---|
| **AN515-57 (this machine)** | 19/55 | 21/58 | **false, 0/0** |
| AN517-52 | 19/55 | 21/58 | **false, 0/0** |
| AN515-58, AN515-47 | 19/55 | 21/58 | true, 0..7317 |
| AN515-54, -51, -45, -44, -43, -42; AN715-51; Predator G3-572, PH315-53 | 19/55 | 21/58 | true, 0..6122 (AN515-42: 5769) |
| Predator PH315-55 | 19/55 | 21/58 | true, 0..5263 / 0..5660 |
| Aspire A715-41G | 19/55 | 21/58 | true, 0..6122 |

Thirteen of the fifteen configurations that use these pairs declare an
independent read scale with a maximum between `5263` and `7317`. The AN515-57
and AN517-52 are the only outliers that disable it. The raw values observed on
this machine (CPU `4225–4615`, GPU `5769–6122`) fall exactly in that numeric
range, including a GPU peak equal to the `6122` maximum used by several sibling
models.

This supports a **configuration omission** explanation for the AN515-57
readback: the hardware exposes a large tach-like value, but the shipped config
does not declare its scale, so NBFC compares it against `0..100` and clamps it.
It does **not** prove the unit is RPM, nor that `6122` is this machine's true
maximum. Those require the calibration protocol in
[`PHYSICAL_VALIDATION_PROTOCOL.md`](PHYSICAL_VALIDATION_PROTOCOL.md).

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

NBFC 0.5.4 with the reviewed access-control patch is now installed. It provides
the JSON status contract, `reset-ec`, and a `0600 root:root` socket reachable
only by the root supervisor. That removes the former access-control blocker,
but the EC readback still cannot confirm that a requested target was reached,
the GPU channel still declares the CPU sensor (though an independent trusted
NVIDIA GPU guard can satisfy software eligibility), and provisional EC tach
candidates have no verified CPU/GPU identity, units or calibration.
The policy therefore continues to refuse physical validation claims. `fan
set`/`fan hold` remain software-policy-gated, and any physical write requires
separate authorization.

## NBFC 0.5.4 upstream source evidence

The 0.5.4 tag was inspected in a temporary clone and then installed from the
reviewed, reproducible integration package. Upstream 0.5.4 adds:

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

**Upstream security did not improve:** unpatched 0.5.4 still hardcodes
`chmod 0666` in `src/server.c` and has no supported socket-permission option.
The installed build is the reviewed patch that adds root-owned config loading,
`SO_PEERCRED`/`SO_PEERGROUPS` authorization and `0600 root:root` creation. The
JSON contract alone still does not justify manual control on this hardware.

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
| NBFC 0.5.4+ | version + `status --json` | Same logical names; independent NVIDIA guard can satisfy GPU temperature policy | Per-index request; protected manual gate | JSON `TargetSpeed` / `CurrentSpeed` (EC percentage, not RPM) | Provisional EC tach candidates only | Patched `0600 root:root` | Installed; EC readback remains uncalibrated |
| hwmon PWM | `pwm*`, `fan*_input` under `/sys/class/hwmon` | Per hwmon channel | Possible if a driver exposes it | PWM value | `fan*_input` when present | sysfs ownership | No Acer fan/PWM hwmon on this kernel |
| `acer_wmi` | loaded | No Acer fan channel on AN515-57 | Not exposed | Not exposed | Not exposed | kernel/hwmon | Active for hotkeys/rfkill; no fan hwmon |
| `nvidia-smi` | NVIDIA driver | GPU telemetry only | Not a fan-control API | Temperature/power; fan `[N/A]` | Not available | user read | Telemetry only; never a fan channel |

This matrix records observed or documented capabilities, not a promise that a
backend is safe to enable. NeoSense does not add a backend merely to increase
coverage.

## Capability matrix for this machine

| Capability | Evidence | Status | Remaining requirement |
|---|---|---|---|
| Fan identity | NBFC config exposes CPU 19/55 and GPU 21/58; NBFC logs distinct raw values for the two channels | Logical identity only | Independent physical fan/airflow evidence |
| Per-fan target | NBFC 0.5.4 installed; separate write registers; no physical write attempted | Config-supported; unvalidated | Explicit authorization and an independent physical reference |
| Target acceptance | Protected NBFC 0.5.4 status contract available; no verified write | Read-only observed | A controlled write with physical verification |
| Backend readback | `IndependentReadMinMaxValues=false`, so 0.5.4 clamps raw 16-bit words (CPU ~4225–4615, GPU ~5769–6122) into the 0..100 write range, reporting `CurrentSpeed=100%` | Invalid as a measurement; configuration omission supported by the family evidence | Independent read scale plus calibration, or another feedback source |
| Physical RPM feedback | EC `0x13/0x14` and `0x15/0x16` give stable provisional candidates (`3896`/`5263`); no independent tachometer or physical mapping | **Provisional only — not feedback** | External calibration and channel-to-fan validation |
| RPM regulation eligibility | RPM sensors have `usable_for_regulation=false`; only temperature sources can reach regulation | Enforced by model, policy and tests | None (by design) |
| CPU temperature mapping | coretemp/x86_pkg_temp; both fans use `@CPU` | Read-only validated | Confirm fan-to-sensor relationship |
| GPU temperature mapping | Trusted NVIDIA GPU die temperature available; NBFC source remains `@CPU` | Eligible with independent guard and explicit warning | Physical fan identity and actuator validation remain separate |
| Automatic-mode recovery | NBFC auto state; supervisor `fan auto` verified in installation lifecycle | Read-only/observed | Recovery behavior under backend failure/crash |
| Access control | Patched NBFC socket `0600 root:root`, `/etc/nbfc/access.json` root-only, ordinary-user connection denied by the kernel; root supervisor connects and reads live status | Read-only validated | None for supervisor-only model |

The only hardware operation performed in previous installation cycles was the
authorized `nbfc set -a` automatic-mode fail-safe during supervisor restarts.
No manual target was sent.

### Raw readback clue

NBFC 0.5.4 logs raw values such as CPU `4225–4615` and GPU `5769–6122` with
“not in range of MinSpeedValueRead/MaxSpeedValueRead”. The source explains
exactly what happens:

- `src/fan.c` derives the effective read range from the write range
  (`MinSpeedValue=0`, `MaxSpeedValue=100`) whenever
  `IndependentReadMinMaxValues` is `false`; the configured
  `MinSpeedValueRead`/`MaxSpeedValueRead` values are then ignored.
- `Fan_UpdateCurrentSpeed` reads up to ten times. If every 16-bit word is
  outside `0..100`, it clamps the value into that range and
  `Fan_FanSpeedToPercentage` reports `100%`.
- Therefore `CurrentSpeed = 100.00%` on this machine is a **clamp artifact**,
  not evidence that either fan runs at full speed.
- `ModelConfig_Validate` only enforces `MinSpeedValueRead != MaxSpeedValueRead`
  when `IndependentReadMinMaxValues` is true, which is why the installed config
  with two zero read values is accepted.
- The NBFC read registers 19/21 are not named in the DSDT EC field list; the
  ACPI `FAN1`/`FAN2` names are 1-bit flags at byte ~171. The register choice is
  therefore empirical, corroborated by the sibling corpus and by the values
  changing independently, not by an ACPI name.
- The warning is emitted at most 30 times per fan per service start
  (`FAN_MAX_READ_WARNINGS`), so a quiet journal after startup does not mean the
  readback became valid.

The EC candidate values are stable and differ by channel, consistent with
16-bit tachometer-like counts, but:

- no unit, scale or min/max calibration is independently known;
- no independent tachometer or physical observation confirms them;
- both logical fans still use `@CPU`, so per-fan physical identity is unproven.

NeoSense exposes the candidates as `Trust::Provisional` with
`usable_for_feedback=false`; they must not be reported as validated RPM. The
register-family evidence makes a configuration omission plausible, but the
unit and true range remain unproven. Establishing them requires a controlled
read-only capture plus a known physical reference, or an external tachometer; any
controlled fan-speed change requires explicit authorization and a recovery
plan. The proposal is in
[`PHYSICAL_VALIDATION_PROTOCOL.md`](PHYSICAL_VALIDATION_PROTOCOL.md); it has
not been executed.

## Future backend acceptance criteria

A future backend should provide stable fan identity, per-fan sensor mapping,
explicit mode/state transitions, raw tachometer or another independent physical
feedback signal, critical-temperature enforcement, EC reset/recovery, and a
way to detect hardware changes. NeoSense exposes manual and profile writes only
for fans that pass the supervisor policy; restricted fans remain automatic and
EC percentages are still reported only as EC percentages.

Persistent custom curves use the same supervisor policy. A validated curve can
be applied to eligible fans; fans with missing/unsuitable sources or blocked
actuator evidence remain automatic. Physical identity, calibration and
actuator-response validation remain separate and unproven.
