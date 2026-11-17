# Physical validation results

This page records operator-attested physical results separately from software
tests and from independent RPM calibration. The supervisor remains the sole
NeoSense hardware writer. Raw machine telemetry is retained outside the
repository.

## CPU fan — retained, not repeated

- Date: 2026-11-15
- Supervisor session: `validation-1794777264574-2`
- Logical channel: 0, `CPU fan`, expected side LEFT
- Sequence: 0% → 50% → 100%, approximately five seconds per step
- Operator observation: **yes**, expected LEFT response
- Recovery: `attempted=true`, `backend_auto=true`
- EC automatic mode independently verified: no; it remains NBFC-managed
- Result: **operator-attested CPU physical response validated**

The 0% step is historical evidence only. The current experimental policy no
longer permits 0%.

## GPU fan — newly validated

- Date: 2026-11-17
- Logical channel: 1, `GPU fan`, expected side RIGHT
- Sequence: **50% → 75% → 100%**, five seconds each
- No 0% target was issued.
- CPU fan remained in NBFC automatic mode for the entire sequence.
- Backend target verified at each step: 50%, 75%, 100%.
- NBFC reported 100% at every step because the raw 16-bit EC word is clamped by
  the effective 0..100 read range; this is not RPM evidence.
- Temperature range during the observed run: CPU 51–67 °C, GPU 50–52 °C.
  Both remained below the 80 °C CPU / 75 °C GPU test ceilings.
- Operator observation: **yes**, the GPU fan/exhaust responded.
- Recovery: `fan.auto` restored both fans to NBFC automatic mode; verification
  succeeded immediately.
- Result: **operator-attested GPU physical response validated**

An earlier complete GPU run was not used for validation because the operator
was not focused on the fan. The observed run above is the validation record.
An initial script attempt before the first complete run issued no fan command;
it failed at the first status read because of a connection-reuse bug.

## Automatic behavior

The normal automatic behavior on this laptop is NBFC's temperature-dependent
software curve:

- After the CPU validation at CPU 65 °C / GPU 56 °C, automatic targets were
  CPU 55% / GPU 60%.
- At idle around CPU 51–53 °C / GPU 50–51 °C, automatic targets were
  CPU 35–45% / GPU 40–50%.
- The targets therefore change with temperature, which is the required normal
  automatic behavior.

`neosense auto` uses the same `fan.auto` / `nbfc set -a` path as the successful
CPU and GPU recovery. It restored both fans to automatic mode after the GPU
test. The EC mode bytes remain NBFC-managed manual codes (`0x0c`/`0x30`); that
is expected for this NBFC configuration and is not treated as a recovery
failure. It does not prove BIOS firmware autonomy, but it is the laptop's
established normal automatic behavior.

## Distinctions

- **Validated:** logical CPU and GPU actuator response observed by the operator.
- **Operator-attested:** physical side mapping CPU→LEFT and GPU→RIGHT.
- **Not validated:** independent airflow/RPM calibration; EC tach words remain
  raw and uncalibrated.
- **Software-tested only:** all non-physical unit/integration tests.
- **Not performed:** any new CPU test, any 0% command, any reset-ec or service
  restart, any deployment of the production supervisor.

## Local evidence

The complete per-step JSON records are kept outside the repository:

- `/tmp/opencode/gpu-validation-20261117-175130.json` — observed validation run
- `/tmp/opencode/gpu-validation-20261117-174940.json` — earlier complete run,
  not used because observation was missed

The supervisor CPU session remains in `/var/lib/neosense/validation/` and is
exported read-only by `neosense validation history`.
