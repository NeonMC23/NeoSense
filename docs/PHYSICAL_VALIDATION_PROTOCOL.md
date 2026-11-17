# Physical fan validation protocol

**Status: CPU AND GPU ACTUATOR RESPONSE PHYSICALLY OBSERVED; INDEPENDENT RPM/AIRFLOW CALIBRATION NOT EXECUTED.**
The CPU logical channel was previously validated at 0/50/100 and accepted. The
GPU logical channel was validated at 50/75/100 on 2026-11-17 using an external
safety-gated run because the installed production supervisor still had the old
0/50/100 sequence. The operator observed both responses. The production
supervisor was not restarted or redeployed. See
[`VALIDATION_RESULTS.md`](VALIDATION_RESULTS.md). The older bounded
Phase-0/Phase-1 proposal is retained later in this document as historical
context.

## 0. Interactive operator-attested workflow (implemented in source)

### Purpose

Establish whether the two logical NBFC channels correspond to the laptop's
physical fans:

- CPU logical fan — expected physical position **LEFT**.
- GPU logical fan — expected physical position **RIGHT**.

LEFT and RIGHT were initial hypotheses. The recorded operator attestation is
CPU→LEFT and GPU→RIGHT; it is not independent airflow/RPM calibration.

### Command

```text
neosense fan validate          # CPU first, then GPU, with separate authorizations
neosense fan validate cpu      # CPU only
neosense fan validate gpu      # GPU only, only after a recorded CPU session
neosense validation history    # read-only evidence export
```

The command requires an interactive terminal for both stdin and stdout. It
refuses `--json`, non-TTY execution, empty input, EOF and malformed answers.
The supervisor never prompts; it owns timing, writes, recovery, session state
and evidence. The CLI only asks the operator and forwards explicit answers.

### Phase A — preflight and authorization

For each fan, the CLI displays fan index/name, logical role, expected side,
other-fan state, CPU/GPU temperatures and freshness, safety blockers, the exact
50% → 75% → 100% sequence and the five-second duration. The experimental minimum is 50%; 0% is forbidden. It then asks an explicit
yes/no question. No session or hardware write exists before an affirmative
answer. Declining records a cancellation without a fan-control command.

### Phase B — supervisor-owned sequence

The supervisor executes exactly:

| Step | Requested target | Maximum duration |
|---|---:|---:|
| 1 | 50% | 5 seconds |
| 2 | 75% | 5 seconds |
| 3 | 100% | 5 seconds |

The supervisor, not the CLI, owns step timing and transitions. It never waits
for operator input between steps and never begins the next step after a
failed preflight, write or verification. A successful target write is followed
by a serialized backend-status refresh before drift detection is re-enabled,
so a sampler snapshot captured before the write cannot be mistaken for the new
step's target. Status reads and target writes share a separate backend lock;
they do not hold the global control lock across backend I/O. Only the selected logical fan is
written; the other fan must remain automatic. Immediately after the final
step it initiates automatic recovery and verifies the backend result.

Timing is recorded with separate fields: command-send time, backend-response
time, hold-start time, hold-finish time, configured hold and actual hold. The
worker uses monotonic time for its hold and a dedicated watchdog deadline;
backend I/O does not hold the global control lock. The watchdog can therefore
invalidate a blocked operation, but a backend call that cannot be interrupted
may remain blocked until its own timeout. The guarantee is therefore: the
requested hold is five seconds; a blocked backend is invalidated and cannot
authorize a later step, but the application cannot forcibly interrupt an
unresponsive kernel/NBFC process.

The ordinary manual-control minimum remains 20%. Experimental validation uses a
separate hard minimum of 50% with fresh CPU/GPU telemetry, separate CPU/GPU
ceilings, protected NBFC access, no conflicting lease/profile/session, a
five-second maximum and supervisor-owned recovery. A 50% request does not prove
50% physical speed; 100% is never evidence of maximum physical RPM.

### Phase C — observation after recovery

Only after recovery has been attempted and recorded does the CLI ask:

> Did you physically observe all three expected CPU fan behaviors on the LEFT?

Backend target/mode verification, backend-reported percentage, EC readback and
physical response are separate evidence levels. A record with a 50% target and
a 100% backend-reported value is target/mode verification only; the reported
value is retained as readback evidence and is not treated as 50%. Physical
response remains false unless an independent measurement mechanism exists.

The answer must be exactly yes or no. Yes records an operator-attested
CPU→LEFT mapping. No records a failed observation and runs read-only
diagnostics; it never retries and never starts the GPU test automatically.
The diagnostic lists hypotheses only and does not assert a cause or a defect.

### Phase D — GPU

The GPU test requires a recorded CPU session with verified recovery and a
separate explicit continuation/authorization. It repeats the same sequence
with expected side RIGHT. Declining or failing it ends the workflow without
further hardware commands.

### Recovery and interruption

The supervisor retains a short validation session TTL, a dedicated monotonic
per-operation/step watchdog and a total session deadline. On supervisor
shutdown it marks stopping, cancels the session with a persisted reason,
recoveries, verifies and joins validation workers before completing shutdown.
The systemd `ExecStopPost` fail-safe remains a last-resort mechanism, not an
orderly cancellation path. It aborts and recovers on stale/missing telemetry,
CPU/GPU ceiling breach, NBFC critical/read-only transition, lost access
control, other-fan mode drift, external control, step/session timeout or
client disappearance. The CLI attempts best-effort abort on Ctrl+C, SIGTERM,
SIGHUP, EOF and IPC errors; the supervisor TTL is the final fallback. After an
ambiguous IPC timeout the result is treated as uncertain and reconciled, never
blindly retried.

### Evidence

The supervisor appends JSONL records under `/var/lib/neosense/validation/`.
Records separate `operator_attestation`, `backend_echo`, `provisional_ec` and
`external_instrument`. No operator answer sets `rpm_calibration_verified`.
`PhysicalIdentity::Unknown` is not changed merely because a target was
accepted.

### Evidence levels

1. Backend target acceptance.
2. Backend-reported target and mode.
3. EC mode evidence.
4. Operator-observed physical response.
5. Operator-attested channel-to-side mapping.
6. RPM calibration status (remains independent and unverified).
7. Actuator validation status (separate from all of the above).

A successful operator observation establishes an operator-attested mapping
only. It does not establish RPM units, absolute speed or independent airflow.
The recorded CPU and GPU results are summarized in
[`VALIDATION_RESULTS.md`](VALIDATION_RESULTS.md).

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
- NBFC reports both channels with `AutoMode=true`. This is NBFC's software
  curve: the installed config sets the EC mode registers to manual
  (`0x22=0x0c`, `0x21=0x30`) and NBFC writes target registers itself. Raw EC
  mode bytes are therefore **not** independent evidence of firmware automatic
  mode. NeoSense reports this as unverified unless a raw EC mode matches the
  community auto code.
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
- On kernel 7.3.1-202 an audited read-only `acer_wmi` candidate exposed
  `hwmon6` with `fan1_input`/`fan2_input`; that was a historical observation.
  On kernel 7.3.4-200 the driver is loaded with `predator_v4=N` and exposes no
  Acer hwmon fan input. Current provisional EC candidates are at
  `0x13/0x14` and `0x15/0x16`; they are not validated feedback.
- The DSDT's EC field list defines `FAN1`/`FAN2`/`FANT` as 1-bit flags at EC
  byte ~171, unrelated to NBFC registers 19/21. SSDT10 contains the Acer
  firmware sensor table (“CPU Fan #1 Speed” in RPM and a duty cycle); the
  read-only hwmon path is now the available source and names no CPU/GPU fan.
- No writable Acer PWM or thermal-profile interface is exposed; NVIDIA reports
  fan speed `[N/A]`. EC tach candidates are provisional observations only.
- Five ACPI `PNP0C0B` fan devices (`\_TZ_.FAN0`–`FAN4`) bind to `acpi-fan` and
  expose binary cooling/power-resource states (`max_state=1`). The audited
  tables contain no `_FIF`/`_FPS`/`_FSL`/`_FST` speed methods. These are not
  RPM, duty cycle, or proof of five physical fans.
- A future run must pass `neosense calibration readiness` first. The report is
  informational only and does not authorize a test. See
  [`CALIBRATION_READINESS.md`](CALIBRATION_READINESS.md).

## 3. What remains unknown

- The physical unit of the NBFC raw value (RPM, tach counts, or a
  vendor-specific scale), and separately whether the provisional EC tach
  candidates have valid units on this exact BIOS (`V1.17`).
- Which physical fan each NBFC and Acer channel drives, and whether the two
  airflow paths are independent. The Acer driver's CPU/GPU sensor IDs are not
  physical proof.
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
- provisional EC tach candidates may be recorded as a **secondary** observation
  only. They are not a substitute for the external tachometer or airflow
  reference, and they do not establish CPU/GPU identity or units;
- the existing root backups verified:
  `/var/backups/nbfc-neosense-20261111-144726` and
  `/var/backups/neosense-2.4.0-20261111-142211`;
- current hashes and the selected config recorded before the test;
- both fans report NBFC automatic mode and the EC mode bytes are recorded;
  do not treat `AutoMode=true` alone as firmware automatic mode; no other fan
  controller active;
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
   Record the provisional EC tach candidates as a **secondary** observation,
   never as the sole reference or as validated RPM.
2. Let the machine idle and then apply a *natural* load increase that the
   firmware controls itself (for example a short compile), while sampling raw
   reads and the external reference. Do **not** run a thermal stress test.
3. Repeat at a lower natural temperature after the machine cools.
4. Fit the observed raw value against the external reference. Report the
   correlation and residuals; do not assume `0..6122` is RPM.

Even a perfect correlation does not prove that writing a target changes the
fan. It only establishes whether the read channel is a usable feedback signal.

## 6. Historic Phase 1 — single-fan bounded step (superseded for the interactive workflow; requires separate authorization)

Test **channel 0 only** (`read=19`, `write=55`) in the first authorized step.
Channel 1 is not touched until its independent NVIDIA temperature guard and
channel-to-physical-fan mapping have been independently established. NBFC's
`@CPU` source is never used as a GPU thermal substitute.

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

## 6a. Measurement options (non-invasive)

| Method | What it can establish | Limitations |
|---|---|---|
| Hot-wire anemometer at each exhaust with a sealed funnel/duct | relative airflow per channel; can distinguish CPU and GPU exhaust | needs a duct and repeated sampling; absolute CFM only with calibration; distance and backpressure affect readings |
| Vane anemometer | coarse relative airflow | high inertia, poor at low flow, bulky duct |
| Optical/laser tachometer | true RPM of one fan | needs line of sight and a reflective marker; the fans are internal, so it requires disassembly and is not non-invasive |
| Acoustic spectrum | presence/change of fan noise | cannot separate two fans reliably or yield RPM; background noise |
| Chassis accelerometer spectrum | fan blade-pass frequencies | indirect, needs calibration, hard to separate two fans |
| EC tach candidates (`0x13/0x14`, `0x15/0x16`) | provisional, unmapped estimates; secondary observation only | no validated units or physical mapping |

Recommendation: use a hot-wire anemometer with a funnel sealed against each
exhaust and sample several points per exhaust. Compare relative flow between
the channels; do not present it as absolute RPM or CFM. Never block an exhaust.
A laser tachometer is only appropriate if the fan hub can be accessed without
opening the laptop (it cannot here). The provisional EC tach candidates are a
secondary check only; they do not satisfy P1 and must not be used as the sole
reference.

Non-invasive airflow measurement can establish **physical identity and relative
response**, but not absolute RPM without an independent calibration. The
current chassis access constraints make an internal optical tachometer
unsuitable. Absolute RPM calibration may therefore remain blocked even after
physical identity is established; do not substitute a sibling-model scale or a
backend percentage.

## 7. Limits and abort conditions

The implemented interactive workflow uses the stricter ceilings of **80 °C CPU**
and **75 °C GPU** from `src/validation.rs`. The limits below are the historical
Phase-1 proposal and are retained for context; they must never override the
implemented ceilings.

Hard limits during Phase 1:

- CPU package temperature at or below 80 °C; GPU at or below 75 °C;
- no NBFC critical flag, no thermal-zone critical trip;
- target step at most 15 points, absolute target within 20–60 %;
- total manual time at most 60 seconds.

Abort immediately and restore automatic mode if any of these occurs:

- CPU above 80 °C or GPU above 75 °C, or any critical flag/trip;
- no external reference change within 10 seconds of the write, or a change in
  the wrong direction;
- the other channel's raw value or target moves with the command;
- unexpected noise, vibration, fan stall or RPM instability;
- any EC error, supervisor error, service restart, suspend/resume or unplug;
- any doubt about what the write affected.

## 8. Recovery and automatic-mode restoration

1. Normal path: `neosense fan auto` through the supervisor, then verify both
   fans report `AutoMode=true` and record the raw EC mode bytes. For the
   interactive workflow, the supervisor performs this recovery before the
   observation question; the validation session TTL also triggers it if the
   CLI disappears. With the
   installed config the expected EC modes are manual (`0x0c`/`0x30`), because
   NBFC's automatic policy owns them; this is not a recovery failure.
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
- both fans report NBFC automatic mode after restoration, with the raw EC
  mode bytes recorded and not misreported as firmware automatic;
- no critical temperature or service event.

## 10. Evidence that would not count

- `CurrentSpeed` reaching the requested percentage (it is the clamped EC
  percentage, not a measurement);
- `nbfc set` returning success;
- a temperature change alone;
- simulation, preview, or a profile curve evaluation;
- a single raw-value change without an independent physical reference;
- a register write followed by a reboot without observation.

## 11. Evidence classes and independence

Keep the following results strictly separate; a later class is not implied by an
earlier one.

| Class | Question answered | Does **not** establish |
|---|---|---|
| **P0 — passive channel identity** | Which firmware/hwmon channel is which, and what are its raw values? | Physical exhaust/fan identity, RPM accuracy, writability |
| **P1 — passive RPM calibration** | How does the Acer RPM value track an independent external reference under ordinary firmware control? | That a write changes the fan, or that the Acer value is absolutely accurate |
| **P2 — controlled actuator response** | Does a bounded write change exactly one physical fan, and which exhaust? | Long-term reliability or a safe production control law |

P0 may be completed from existing read-only telemetry. P1 requires an external
reference but no fan write. P2 requires separate explicit authorization for a
bounded fan target and must follow this protocol's limits.

The Acer firmware sensor IDs are:

| hwmon | driver sensor ID | firmware name | current physical conclusion |
|---|---:|---|---|
| `fan1_input` (historical) | 2 | CPU Fan #1 Speed (SSDT10, unit RPM) | not exposed on kernel 7.3.4; provisional EC candidate only |
| EC `0x15/0x16` | 6 (candidate) | provisional tach estimate | not validated; no physical exhaust mapping |

## 12. Instrument requirements and uncertainty

An instrument is acceptable only if its calibration is traceable or its
relative response is sufficient for the question being answered.

- **Optical/laser tachometer (preferred for absolute RPM):** measurement range
  covering at least 500–8000 RPM, accuracy at least ±0.05 % of reading or
  ±1 RPM (whichever is larger), update rate ≥1 Hz, and a reflective marker
  suitable for the fan hub. Record the instrument model, serial number,
  calibration date and stated uncertainty. Do **not** open the laptop or place
  anything into the fan assembly to obtain line of sight.
- **Hot-wire anemometer with a sealed exhaust funnel (preferred for
  non-invasive relative airflow):** response time ≤1 s, resolution ≤1 % of
  reading, calibrated against a known reference or used only for relative
  comparisons, with a funnel/duct that seals against the exhaust without
  blocking it. Record ambient temperature, barometric pressure and the
  funnel geometry. Absolute CFM is not required for P1; relative airflow can
  establish which exhaust responds.
- **Vane anemometer / acoustic / accelerometer methods:** acceptable only as
  corroboration; they are not sufficient alone for absolute RPM or physical
  identity.
- **EC tach candidates:** always provisional and secondary. They are not
  validated units and are not independent physical evidence.

Report the combined standard uncertainty of the external reference. Do not
subtract or smooth the Acer values to improve agreement.

## 13. Safe measurement access

- Use only the existing exhaust grilles; do not disassemble the chassis, remove
  the bottom cover, or insert probes, wires or objects into the fan assembly.
- Do not block, seal permanently or restrict either exhaust. A measurement
  funnel may surround an exhaust only if it leaves the airflow path open and is
  removed immediately after the sample.
- Keep the laptop on AC power, battery above 50 %, ambient temperature stable,
  no pending suspend, and no other thermal controller active.
- Ensure both fans are in automatic mode and no manual/preset lease is active
  before any baseline capture.
- Stop if the machine is moved, a cable is disturbed, or either exhaust becomes
  obstructed.

## 14. Passive P1 procedure — repeated calibration points

Perform this only with the machine under ordinary firmware control; it requires
no fan write.

1. Confirm both fans are automatic and record:
   - `neosense fan status --json` (targets, EC values, mode);
   - `neosense sensors list --json` (provisional EC tach candidates, CPU package, GPU);
   - kernel, `acer_wmi` srcversion, module hash and DMI model;
   - external reference model/serial/calibration and ambient conditions.
2. Attach the external reference to **one exhaust at a time** without
   obstructing it. Record which exhaust is instrumented.
3. Capture at least **five stable plateaus** spanning the ordinary operating
   range. A plateau means the external reference is stable within its stated
   uncertainty for at least 10 seconds. Do not induce a plateau with a stress
   test; use natural changes in ordinary use, or wait for firmware-controlled
   changes.
4. At each plateau, take at least **five samples at 5-second intervals**:
   external reference, EC tach candidates, CPU package, GPU, timestamps,
   freshness and any read error.
5. Repeat the full set with the instrument on the other exhaust. If the
   instrument cannot measure both exhausts safely, record that limitation; do
   not infer the unmeasured path.
6. After the run, restore the instrument, confirm both fans are automatic and
   confirm no critical event occurred.

Analysis:

- Fit external reference vs provisional EC candidates separately. `neosense
  calibration analyze --input FILE --json` performs this offline analysis and
  applies the acceptance criteria below. It does not access hardware. Report
  slope, intercept, coefficient of determination, residual standard deviation
  and 95 % confidence intervals.
- Report the raw paired samples and the measurement uncertainty; do not
  publish only a correlation.
- For absolute RPM calibration, require a slope of 1.00 ± 0.05, an intercept
  within ±50 RPM, and R² ≥ 0.99 across at least five plateaus, with no
  systematic curvature. If these are not met, report the calibration as
  unverified and retain the raw data.
- For relative airflow only, require a monotonic, repeatable response and
  clearly label the result **relative**, not RPM-calibrated.

Passing P1 does not establish physical identity. A channel can track an
external reference and still be mislabeled or non-independent.

## 15. P0/P1 evidence matrix for physical identity

Physical identity may be declared only when the evidence distinguishes the two
paths, not merely when one channel moves.

| Observation | Fan 1 | Fan 2 | Interpretation |
|---|---|---|---|
| External airflow/RPM changes at CPU exhaust only | changes | unchanged | candidate Fan 1 = CPU path, if repeated and the other exhaust is instrumented |
| External airflow/RPM changes at GPU exhaust only | unchanged | changes | candidate Fan 2 = GPU path, if repeated and the other exhaust is instrumented |
| Both channels track both exhausts | changes | changes | no independent mapping; keep both neutral |
| One channel changes but the instrumented exhaust is uncertain | changes | unknown | `unknown`; repeat with unambiguous access |
| Correlation exists but no external path measurement | any | any | **not** physical identity |

Require at least three independent repeats, a stable baseline, and the other
exhaust measured sufficiently to show no corresponding change. If either
exhaust cannot be measured safely, the result is `logical_only`/`unknown`, not
`physically_verified`.

## 16. P2 controlled actuator-response criteria (future authorization)

Do not perform P2 until P0/P1 are complete and a separate written authorization
exists. When authorized, P2 must use the bounded single-fan step in section 6
and must satisfy all of the following before physical control is considered
validated:

- a pre-registered target step of at most 15 percentage points within 20–60 %;
- an external reference showing a response at exactly one exhaust;
- the other exhaust and the other Acer channel remaining within measurement
  noise;
- the response repeating on at least two independent runs;
- both fans returning to automatic mode and the pre-test behavior;
- no critical temperature, EC error, service restart or unexpected register
  change;
- the full raw record, instrument uncertainty and analysis retained.

A backend-accepted target, `CurrentSpeed` change, temperature change or a
single un-repeated observation is **not** sufficient. Passing P2 would validate
only the tested bounded step on this exact hardware/BIOS; it would not create a
general control law.

## 17. Abort and stop rules (expanded)

Abort the passive run if:

- either exhaust is obstructed or the instrument is unstable;
- CPU package exceeds 80 °C or GPU exceeds 75 °C;
- an NBFC critical flag/trip, sensor loss, stale snapshot, or service restart
  occurs;
- the external reference and both Acer channels move together in a way that
  cannot be attributed to one path;
- the machine suspends, resumes, loses AC power or is moved;
- any measurement uncertainty or channel assignment becomes ambiguous.

Abort the future P2 run if any section 7 condition occurs, or if the other
channel/exhaust responds. Restore automatic mode immediately; do not continue
to "confirm" a result after an abort.

## 18. Measurement worksheet

Print this template and record values by hand or in a local file outside the
repository. Do not commit raw machine telemetry.

### Session header

| Field | Value |
|---|---|
| Date/time (timezone) | |
| Operator | |
| Laptop / DMI product | Acer Nitro AN515-57 |
| BIOS version | |
| Kernel | |
| `acer_wmi` srcversion / module SHA-256 | |
| NBFC version / selected config | |
| NeoSense version / commit | |
| Instrument model / serial | |
| Instrument calibration date / uncertainty | |
| Ambient temperature / pressure | |
| AC power / battery % | |
| Both fans automatic? | yes / no |
| Other thermal controllers active? | |

### Passive plateau record (repeat per plateau and per exhaust)

| Field | Sample 1 | Sample 2 | Sample 3 | Sample 4 | Sample 5 |
|---|---|---|---|---|---|
| Timestamp | | | | | |
| Instrumented exhaust (CPU/GPU/other) | | | | | |
| External reference (RPM or airflow) | | | | | |
| External uncertainty | | | | | |
| EC tach candidate 0 (`0x13/0x14`) | | | | | |
| EC tach candidate 1 (`0x15/0x16`) | | | | | |
| CPU package (°C) | | | | | |
| GPU (°C) | | | | | |
| NBFC fan 0 target / EC | | | | | |
| NBFC fan 1 target / EC | | | | | |
| Snapshot source/freshness | | | | | |
| Read error | | | | | |

### Plateau summary

| Plateau | Instrumented exhaust | External mean ± u | Acer F1 mean | Acer F2 mean | Stable ≥10 s? | Notes |
|---|---|---|---|---|---|---|
| 1 | | | | | | |
| 2 | | | | | | |
| 3 | | | | | | |
| 4 | | | | | | |
| 5 | | | | | | |

### Analysis record

| Field | EC candidate 0 | EC candidate 1 |
|---|---|---|
| Paired samples (n) | | |
| Slope (external vs Acer) | | |
| Intercept | | |
| R² | | |
| Residual SD | | |
| 95 % CI | | |
| Absolute-RPM criteria met? | yes / no / not applicable | yes / no / not applicable |
| Physical identity evidence | CPU / GPU / unknown | CPU / GPU / unknown |
| Identity confidence and reason | | |

### Optional analysis command

Save the paired samples in a local JSON file outside the repository and run:

```bash
neosense calibration analyze --input p1-samples.json --json
```

The command applies the P1 criteria and returns exit 0 only when the **RPM**
reference and both channels pass. A relative-airflow reference can never claim
absolute RPM calibration. Attach the raw input and report to the session record.

### Sign-off

- P0 logical channel identity: confirmed / unknown
- P1 absolute RPM calibration: verified / unverified
- Physical CPU/GPU response: operator-attested / independent airflow mapping unverified
- P2 controlled actuator response: not performed / passed / failed
- Both fans automatic after the session: yes / no
- Critical events or deviations:
- Reviewer / date:

**No result in this worksheet may be promoted to a hardware fact without the
raw samples, independent reference and reviewer sign-off.**

## 19. Explicitly prohibited during this protocol unless separately authorized

- writes to any register other than the single tested write register;
- changing `MinSpeedValueRead`/`MaxSpeedValueRead` or any live mapping;
- starting a stress test;
- BIOS/firmware/EC firmware changes;
- stopping or modifying unrelated thermal services;
- testing both channels in one session.
