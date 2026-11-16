# Calibration readiness and physical-evidence gate

**Status: READ-ONLY IMPLEMENTED. NO PHYSICAL TEST IS AUTHORIZED OR EXECUTED.**

This document describes the evidence required before a future physical fan
calibration, the read-only readiness command, and the current result on this
machine. It does not authorize a fan command, an EC write, an ACPI/AML call, a
stress test or a thermal experiment.

## 1. Command and JSON

```text
neosense calibration readiness
neosense calibration readiness --json
```

Schema: `neosense.calibration_readiness/v1`.

The command always sets:

- `informational_only: true`
- `test_authorized: false`
- `protocol.test_authorized: false`

Exit status is `0` only when every recorded prerequisite is satisfied, and `6`
for `blocked` or `partially ready`. Even exit `0` is informational and does not
authorize a test.

States:

| State | Meaning |
|---|---|
| `blocked` | At least one essential prerequisite is missing or failed |
| `partially_ready` | Essential prerequisites are present, but identity/RPM/actuator/recovery evidence remains unverified |
| `ready_for_separately_authorized_test` | All recorded prerequisites are satisfied; separate explicit authorization is still mandatory |

The evaluator fails closed. Unknown, stale, contradictory or unavailable
evidence is never converted to success.

## 2. Requirements

| ID | Requirement | What counts as evidence |
|---|---|---|
| `runtime_backend` | Runtime EC backend verified | Open NBFC descriptor matching a known EC endpoint (`ec_sys`, `acpi_ec`, `dev_port`) |
| `logical_channel` | Logical CPU/GPU channels identified | NBFC status has distinct logical CPU and GPU names/indexes; this is not physical identity |
| `physical_identity` | Physical fan identity independently verified | Per-fan external airflow or tachometer mapping evidence |
| `independent_rpm` | Independent RPM/reference available | Fresh trusted `fan*_input` RPM or a calibrated external reference; raw EC words do not count |
| `cpu_telemetry` | Trusted CPU telemetry | Fresh trusted CPU package temperature |
| `gpu_telemetry` | Trusted GPU telemetry | Fresh trusted GPU die temperature when a GPU fan is configured |
| `actuator_semantics` | Validated actuator semantics | Recorded single-fan physical response, not backend target acceptance |
| `recovery_path` | Recovery path verified | Recorded automatic recovery with backend and raw EC evidence |
| `temperature_ceilings` | Ceilings valid | CPU ≤ 80 °C, GPU ≤ 75 °C, configured limits valid |
| `experimental_floor` | Experimental floor enforced | Minimum 50%; sequence 50/75/100%; no 0% step |

The protocol also records: five-second maximum step duration, one fan at a time,
other fan automatic, automatic recovery required, and separate explicit
authorization required.

## 3. Current result on this machine

Environment: Acer Nitro AN515-57, BIOS/EC `V1.17` / EC firmware `1.10`, board
`Scala_TLS V1.17`, Nobara Linux 44, kernel
`7.3.4-200.nobara.fc44.x86_64`.

| Requirement | Current status | Evidence |
|---|---|---|
| `runtime_backend` | Missing in the production supervisor | An isolated root supervisor verified `ec_sys`, but the installed production build predates the runtime probe |
| `logical_channel` | Satisfied | NBFC reports `CPU fan` index 0 and `GPU fan` index 1 |
| `physical_identity` | Unverified | No independent per-fan mapping evidence |
| `independent_rpm` | Missing | Only provisional raw EC words; no independent tachometer or calibrated airflow reference |
| `cpu_telemetry` | Satisfied | Trusted coretemp package reading |
| `gpu_telemetry` | Satisfied | Trusted NVIDIA GPU die reading |
| `actuator_semantics` | Unverified | No recorded single-fan physical response |
| `recovery_path` | Unverified | A recorded recovery showed backend automatic mode, but raw EC automatic mode was not independently verified |
| `temperature_ceilings` | Satisfied | CPU 80 °C, GPU 75 °C |
| `experimental_floor` | Satisfied | 50/75/100%, no 0% step |

**Result: `blocked`.**

Exact blockers:

1. The installed production supervisor does not yet report runtime backend
   evidence. The branch implementation verifies `ec_sys` when run by the root
   supervisor; deployment is a separate authorization.
2. No independent RPM or calibrated airflow reference exists.
3. Physical identity, actuator response and recovery evidence are unverified.

## 4. Raw EC evidence

NBFC reads CPU `0x13` and GPU `0x15` as 16-bit words. With
`IndependentReadMinMaxValues=false`, it compares those words against the
effective `0..100` write range, clamps them and reports `CurrentSpeed=100%`.
The raw words observed in read-only samples ranged from roughly `2500` to
`5000`, but this is not a calibrated range.

The raw value is classified as:

- **Proven:** a live 16-bit EC word in the register NBFC reads.
- **Strongly supported:** tachometer-like and varying independently per
  channel, based on NBFC source, sibling configuration ranges and repeated
  read-only samples.
- **Unproven:** engine RPM, fan RPM, a period, a pulse count, or a specific
  vendor percentage.
- **Not proven by name or register adjacency:** which physical fan is attached
  to channel 0 or channel 1.

No conversion formula is applied. The value remains `unit: "raw"` with
`verified_rpm: false`.

## 5. New ACPI fan finding

Linux exposes five ACPI fan devices on this machine:

```text
\_TZ_.FAN0  PNP0C0B  -> /sys/devices/platform/PNP0C0B:00
\_TZ_.FAN1  PNP0C0B  -> /sys/devices/platform/PNP0C0B:01
\_TZ_.FAN2  PNP0C0B  -> /sys/devices/platform/PNP0C0B:02
\_TZ_.FAN3  PNP0C0B  -> /sys/devices/platform/PNP0C0B:03
\_TZ_.FAN4  PNP0C0B  -> /sys/devices/platform/PNP0C0B:04
```

They bind to `acpi-fan` and appear as `cooling_device0`–`cooling_device4`
with `type=Fan`, `max_state=1`, `cur_state=0` at idle. The static SSDT14
devices contain only `_HID`, `_UID` and `_PR0` power resources; there are no
`_FIF`, `_FPS`, `_FSL` or `_FST` fan-speed methods in the audited tables.

Therefore these devices expose a binary firmware power/cooling state, **not
RPM, duty cycle or airflow**. They must not be used to infer a physical fan
count, a mapping or an RPM value. Reading their `cur_state` is read-only;
writing it is prohibited by the safety rules.

## 6. Exact-model documentation and part-number research

| Source | Finding | Evidence strength |
|---|---|---|
| Local DMI | Board `Scala_TLS V1.17`; BIOS `V1.17`; EC firmware `1.10`; chassis type 10 | Proven on this machine |
| ACPI tables and Linux sysfs | Five PNP0C0B fan power-resource devices; no fan-speed methods | Proven on this machine |
| Acer user manual/listing references | AN515-57 uses independent CPU/GPU fan controls in NitroSense | Strong, but not physical identity |
| Repair-forum board references | AN515-57 variants are associated with `GH53G LA-L191P`; a Scala_TLS board is reported on AN515-57 | Community lead, not proof for this exact revision |
| Retail fan listings | CPU fan part numbers `23.Q7KN2.001`/`23.QB9N2.001`; GPU listings `DC28000X0F0`/`DC28000X1F0` | Retail lead only; not an official service-manual confirmation |
| Acer service-manual search | No verified AN515-57 service manual with exact fan assembly and connector pinout was located | Unknown; do not claim |

The exact fan assembly part numbers, connector pinout, tachometer wiring and
board revision mapping remain **unknown**. No schematic was downloaded or
executed, no chassis was opened, and no fan was removed.

## 7. External measurement options

| Method | Can distinguish two fans? | Absolute RPM? | Safe without opening? | Assessment |
|---|---|---|---|---|
| Hot-wire anemometer with sealed exhaust funnel | Yes, one exhaust at a time | Relative airflow; absolute only with calibration | Yes, if the exhaust is not blocked | **Recommended primary method** |
| Vane anemometer | Coarse only | No | Yes | Corroboration only |
| Optical/laser tachometer | One fan at a time | Yes | No: internal line of sight required | Not suitable for this mission |
| Microphone/acoustic spectrum | Difficult; blade-pass frequencies overlap | No without calibration | Yes | Corroboration only |
| Chassis accelerometer | Difficult | No without calibration | Yes | Research only |
| Thermal camera | Shows heat patterns, not airflow/RPM | No | Yes | Cannot establish fan identity or RPM |
| USB measurement device | Depends on the instrument | Depends on calibration | Yes | Acceptable only with a documented reference |

Recommended future method: a hot-wire anemometer with a removable sealed funnel
at one exhaust at a time, using repeated stable samples and recording ambient
conditions and uncertainty. It can establish **which exhaust responds** and
relative airflow. It does not by itself establish absolute RPM. An independent
optical tachometer would require internal access and is not acceptable under the
current safety constraints. Therefore absolute RPM calibration may remain
blocked even after physical identity is established by airflow.

## 8. Future calibration protocol (not executed)

No step below is authorized by this document.

1. Run `neosense calibration readiness --json`; require a current, reviewed
   report and a separate explicit authorization.
2. Confirm both fans are automatic, temperatures are below 80 °C CPU / 75 °C
   GPU, telemetry is fresh/trusted, and automatic recovery is available.
3. Attach the external airflow reference to one exhaust only, without blocking
   it. Record the baseline. Do not write a fan target.
4. Under ordinary firmware control, collect at least five stable plateaus and
   five samples per plateau. Compare raw EC channel values with the external
   reference. This is passive P0/P1 evidence, not actuator validation.
5. Only after identity and an independent reference are established may a
   separately authorized single-fan test use the supervisor-owned 50/75/100%
   sequence. Keep the other fan automatic, hold each step for five seconds,
   abort on any ceiling/telemetry/recovery anomaly, and restore automatic mode
   before observation.
6. Record target, backend readback, raw EC word, external airflow/RPM,
   temperatures, timestamps, uncertainty, recovery evidence and operator notes
   separately. Never promote backend readback or operator observation to RPM.
7. Repeat and report uncertainty. A single response is not calibration.

## 9. Next safe engineering step

Deploying the new runtime probe to the production supervisor is a separate
authorization. Until then, the readiness command correctly reports the runtime
backend as missing rather than inferring it. After deployment, the next safe
step is passive external-airflow measurement at each exhaust with both fans
under firmware control; no fan target is required.
