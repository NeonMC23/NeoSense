# Physical fan validation protocol (proposal only — not executed)

**Status: NOT AUTHORIZED, NOT EXECUTED.** This document defines the smallest
experiment that could turn the current read-only evidence into a physically
validated result. No part of Phase 1 has been run. Phase 0 has also not been
run on this host; only passive observations already recorded in
[`HARDWARE.md`](HARDWARE.md) exist.

## 1. Exact unresolved hypothesis

> Registers 19/55 and 21/58 are two independent tach-like channels on the Acer
> Nitro AN515-57; writing a bounded fan-speed target to one write register
> changes the corresponding physical fan, and its read register reports a
> proportional tach-like value.

Sub-hypotheses, each needing separate evidence:

1. The value read from registers 19 and 21 is a tach-like quantity (not a
   percentage and not a control-loop error), with a stable scale on this model.
2. Channel 0 (`read=19`, `write=55`) drives the CPU-side fan and channel 1
   (`read=21`, `write=58`) drives the GPU-side fan. The logical labels and the
   family-wide register convention suggest this, but no physical observation
   exists.
3. A bounded write to one channel changes only that fan; the other channel and
   unrelated EC state remain unaffected.

## 2. What is already established (read-only)

- NBFC 0.5.4 is installed; the supervisor alone reaches the `0600 root:root`
  socket; the CLI/TUI use IPC.
- Both channels are in automatic mode.
- The selected config has `IndependentReadMinMaxValues=false`, so NBFC compares
  raw 16-bit reads against the write range `0..100` and clamps out-of-range
  values. `CurrentSpeed=100%` is therefore a clamp artifact.
- Raw values observed in the NBFC journal (14:47:53–55) were CPU `4225–4615`
  and GPU `5769–6122`; logging stops after 30 warnings per fan, so absence of
  newer warnings is not evidence.
- Fourteen sibling Acer Nitro/Predator configurations use the same register
  pairs. Thirteen declare `IndependentReadMinMaxValues=true` with
  `MaxSpeedValueRead` between `5263` and `7317`; the AN515-57 and AN517-52 are
  the only ones that disable the independent read scale.
- There is no `fan*_input`, no `pwm*`, no Acer hwmon fan channel, and NVIDIA
  reports fan speed `[N/A]`.
- The DSDT's EC field list defines `FAN1`/`FAN2`/`FANT` as 1-bit flags at EC
  byte ~171, unrelated to NBFC registers 19/21. SSDT10 contains an Acer
  firmware sensor table with a **“CPU Fan #1 Speed” in RPM** and a CPU fan duty
  cycle, but no read-only Linux interface is currently exposed and the table
  names no GPU fan.

## 3. What remains unknown

- The physical unit of the raw value (RPM, tach counts, or a vendor-specific
  scale) and the true min/max on this exact BIOS (`V1.17`).
- Which physical fan each channel drives, and whether the two airflow paths are
  independent.
- Whether a write to the write register is accepted and produces a physical
  response on this model.
- Whether the configuration's register mapping is correct for this hardware
  revision.

## 4. Prerequisites

All of the following are required before Phase 1:

- separate, explicit written authorization for a manual fan target and the
  associated EC write on this machine;
- an independent, trustworthy fan reference: a laser/optical tachometer with a
  reflective marker on the fan hub, or a calibrated airflow meter at the
  exhaust. A phone app, acoustic estimate or `CurrentSpeed` is not acceptable;
- a root terminal (the experiment is not performed by the NeoSense CLI);
- a decision on whether the firmware RPM descriptor may be used as a secondary
  reference. Reading it requires a kernel module or driver configuration change
  (`acpi_call`, `acer_wmi predator_v4`) that is **not authorized** by this
  protocol; do not enable it as part of the experiment without separate
  approval;
- the existing root backups verified:
  `/var/backups/nbfc-neosense-20261111-144726` and
  `/var/backups/neosense-2.4.0-20261111-142211`;
- current hashes and the selected config recorded before the test;
- both fans confirmed in automatic mode, no other fan controller active;
- laptop on AC power, battery above 50 %, no pending suspend or firmware
  update, no unrelated thermal service changes;
- the machine idle (no stress test, no benchmark, no game), with thermal
  headroom below.

## 5. Phase 0 — read-only reference capture (no fan writes)

Purpose: calibrate the read scale and confirm that the channels move with
physical speed, without issuing a target.

1. Record the baseline: `nbfc status --json`, both raw EC channels via the
   NBFC journal (and, only with separate approval, a read-only EC read), CPU/GPU
   temperatures, and the external tachometer/airflow reading at each exhaust.
   If an authorized read-only firmware interface is available, record its
   CPU-fan RPM descriptor as a **secondary** reference, never as the sole one.
2. Let the machine idle and then apply a *natural* load increase that the
   firmware controls itself (for example a short compile), while sampling raw
   reads and the external reference. Do **not** run a thermal stress test.
3. Repeat at a lower natural temperature after the machine cools.
4. Fit the observed raw value against the external reference. Report the
   correlation and residuals; do not assume `0..6122` is RPM.

Even a perfect correlation does not prove that writing a target changes the
fan. It only establishes whether the read channel is a usable feedback signal.

## 6. Phase 1 — single-fan bounded step (requires separate authorization)

Test **channel 0 only** (`read=19`, `write=55`), because it is the channel whose
role mapping (`@CPU`) is at least plausible on this platform. Channel 1 is not
touched because its `@CPU` source mapping is known to be wrong.

1. Confirm Phase 0 produced a usable external reference and a stable baseline.
2. Confirm both fans are automatic and temperatures are below the limits in
   section 7.
3. Record a full read-only EC snapshot and `nbfc status --json`.
4. Issue one bounded manual target for fan 0 through the existing authorized
   supervisor path, not by a direct ad-hoc EC write:
   - start from the current automatic target;
   - choose a single step of at most 15 percentage points;
   - do not exceed 60 % and do not go below 20 %;
   - hold for at most 60 seconds.
5. During the hold, record the external reference at the CPU exhaust, the raw
   read value, the backend target/reported values, and both temperatures.
6. Immediately restore automatic mode and verify both fans report automatic.

This is a single step, not a sweep. It answers "does anything physical change
and is it confined to one channel", not "what is the full curve".

## 7. Limits and abort conditions

Hard limits during Phase 1:

- CPU package temperature below 85 °C; GPU below 80 °C;
- no NBFC critical flag, no thermal-zone critical trip;
- target step at most 15 points, absolute target within 20–60 %;
- total manual time at most 60 seconds.

Abort immediately and restore automatic mode if any of these occurs:

- CPU ≥ 85 °C or GPU ≥ 80 °C, or any critical flag/trip;
- no external reference change within 10 seconds of the write, or a change in
  the wrong direction;
- the other channel's raw value or target moves with the command;
- unexpected noise, vibration, fan stall or RPM instability;
- any EC error, supervisor error, service restart, suspend/resume or unplug;
- any doubt about what the write affected.

## 8. Recovery and automatic-mode restoration

1. Normal path: `neosense fan auto` through the supervisor, then verify both
   fans report `AutoMode=true`.
2. If the supervisor is unresponsive: restart `neosensed` (its authorized
   ExecStopPost fail-safe returns NBFC to automatic mode), then verify.
3. If NBFC itself is stuck: restart `nbfc_service` (the preserved unit has no
   `reset-ec`) and verify automatic mode.
4. Last resort: reboot. Do not issue arbitrary EC writes to recover.
5. Keep the machine under observation for at least five minutes after recovery
   and confirm both fans and temperatures return to the pre-test range.

## 9. Evidence that would count

- external tachometer/airflow change at exactly one exhaust, correlated with
  the bounded target and with the raw read value; a firmware RPM descriptor is
  corroborating evidence only;
- no change in the other channel's raw value beyond measurement noise;
- a before/after read-only EC dump showing no unexpected register changes;
- both fans confirmed automatic after restoration;
- no critical temperature or service event.

## 10. Evidence that would not count

- `CurrentSpeed` reaching the requested percentage (it is the clamped EC
  percentage, not a measurement);
- `nbfc set` returning success;
- a temperature change alone;
- simulation, preview, or a profile curve evaluation;
- a single raw-value change without an independent physical reference;
- a register write followed by a reboot without observation.

## 11. Explicitly prohibited during this protocol unless separately authorized

- writes to any register other than the single tested write register;
- changing `MinSpeedValueRead`/`MaxSpeedValueRead` or any live mapping;
- starting a stress test;
- BIOS/firmware/EC firmware changes;
- stopping or modifying unrelated thermal services;
- testing both channels in one session.
