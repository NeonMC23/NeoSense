# NeoSense — Product roadmap and status

**Updated:** 2026-11-10
**Current version:** 2.4.0 development

Statuses are evidence-based. A feature is not complete merely because code or
documentation exists.

## Complete and validated by automated tests

| Area | Evidence |
|---|---|
| NBFC 0.5.3 human status parsing | Fixtures and parser tests |
| NBFC 0.5.4 JSON status parsing | Fixture tests |
| Sensor roles/provenance/trust | hwmon/thermal/NVIDIA fixtures |
| Missing/invalid sensor handling | Unit tests; no zero substitution |
| Supervisor state classification | Unit tests for healthy, missing and permission cases |
| IPC request/response and permission errors | Unit and CLI tests |
| Warning severity and consolidation | Unit/CLI tests |
| Profile compatibility | Unit tests and CLI/JSON tests |
| TUI rendering and navigation | TestBackend at 40/80/100/160 columns; scroll and empty states |
| Supervisor polling efficiency | 12s fake-backend measurement: version/config calls reduced 8/8 -> 1/1; `status` remained live; RSS difference not established |
| CLI JSON and exit codes | 41 CLI tests; schemas, NDJSON watch and option-rejection coverage |
| Installer dry-runs | Temporary prefix/systemd/runtime/state; unsafe paths rejected |
| Isolated root install/rollback/uninstall | `packaging/test-installer-isolated.sh` passed in user namespace |
| Release build | `cargo build --release --locked` |
| Release archive | `packaging/build-release.sh` generated tarball and checksums |
| Prebuilt archive install | `packaging/test-release-archive.sh` extracts and installs without Cargo |
| Exact final archive systemd lifecycle | release archive checked against `SHA256SUMS`, Fedora 44/systemd 259: fresh install, READY, socket `0660`, watchdog, clean stop, SIGKILL recovery, ExecStopPost |
| Non-root IPC in disposable container | exact archive: group member succeeded; outsider exited 4 (fake NBFC backend) |
| Container in-place upgrade/uninstall | exact archive: reinstall created a backup; injected daemon-reload failure rolled back; uninstall preserved unrelated NBFC/config files |
| Cross-version upgrade | rebuilt historical 2.2.0 fixture -> current 2.3.0: binary/unit replacement, backup integrity, state and unrelated-file preservation, injected-failure rollback |
| Active-supervisor upgrade restart | 2.2.0 -> 2.3.0: automatic restart, READY/socket/IPC verification, injected first-restart failure restored 2.2.0 and restarted previous service; `--no-restart` tested |
| Multi-fan/manual safety | fake backend: manual set exit 8 with no `set -f`; supervisor group authorization enforced |
| Thermal policy engine | deterministic curves, interpolation, clamping, hysteresis, rate limiting, staleness, mapping, preview/simulation/no-write tests |
| Historical fixed presets | Silent 25%, Balanced 60%, Performance 80%, Turbo 100%; supervisor preflight, multi-fan lease, fake-backend writes, automatic recovery |
| Historical v1.0.1 comparison | `docs/HISTORICAL.md` source-backed mapping of restored/redesigned/blocked behavior |
| Simulated profile IPC | `profile.select`/`profile.get`/`profile.clear`; fake-backend test proves zero `set_speed` calls |
| Persistent custom profiles | versioned JSON, strict validation, atomic writes + fsync, write lock, backup, corruption recovery, migration, CRUD IPC, CLI/TUI editor, fake-backend preview/simulation tests |
| Custom-profile CLI/TUI | isolated-store lifecycle tests; TUI rendering and editor-template tests; supervisor-owned CRUD test |
| Per-fan identity evidence | read-only NBFC config parser and `hardware identity`; distinct CPU/GPU register pairs, shared `@CPU` source, invalid readback range surfaced in CLI/TUI/diagnostics; tests for parser and warning logic |
| 2.4.0 host installation | repaired artifact `61423d0` installed 2026-11-11 16:08; installed binaries byte-identical to the artifact; root-protected backup, READY/status, watchdog timestamp advancing, `NRestarts=0`, StateDirectory/`0600` profile store |
| Readback-scale accuracy | `hardware identity` reports the effective read range, its source (`independent`, `write_range`, `invalid`) and whether an independent scale is declared; warnings name the clamp behaviour instead of a generic invalid range |
| NBFC access-control gating | identity-aware socket classification (mode/owner/group membership/ACL/connectability), operation-time revalidation, recovery on lost access, security warning in status/TUI/diagnostics; `0660 root:neosense` rejected for ordinary IPC users; upstream 0.5.4 and master still `0666` |
| NBFC upstream access-control patch | reviewed reproducible 0.5.4 patch (base `9da010d`): configurable owner/group/mode, secure `0600 root:root` default, `SO_PEERCRED`/`SO_PEERGROUPS` control allowlist, no permission window, tests/build passing; **installed on the host 2026-11-11** |
| Supervisor-only backend access | CLI/TUI build their runtime view from supervisor IPC instead of spawning `nbfc`; diagnostics no longer misreport an ordinary-user denial as a backend outage; regression tests cover IPC-only state and missing supervisors |
| NBFC version revalidation | supervisor re-probes the client version when `nbfc_service` restarts or the cache ages out; write gates always re-probe live and fail closed; validated on the host after deployment (`NBFC 0.5.4`, `protected_manual=true`) |
| Honest physical state | diagnostics separate “manual path eligible”, “physical feedback” and “physical control validated”; backend capability is never reported as physical control |
| Profile refresh race | profile evaluation now uses the selection current at commit time; a concurrent refresh can no longer overwrite a simulation selection; bounded concurrency regression test |
| Acer register-family audit | 323 NBFC configs audited; 13 of 15 models sharing `19/55` and `21/58` declare an independent read scale (max 5263–7317); AN515-57 is an outlier |

## Validated on the local machine (read-only)

| Area | Result |
|---|---|
| Hardware detection | Acer Nitro AN515-57, i5-11400H, RTX 3050 |
| OS/kernel | Nobara 44, 7.3.1-202.nobara.fc44.x86_64 |
| NBFC | patched 0.5.4 (`/usr/bin/nbfc`), active, two fans; socket `0600 root:root`; root-only policy in `/etc/nbfc/access.json` |
| CPU/GPU/NVMe/Wi-Fi/ACPI roles | Correct separation |
| GPU fan mapping | Both fans use `@CPU`; GPU manual control blocked |
| EC readback | raw 16-bit words clamped into the 0..100 write range; `100%` is a clamp artifact. The config omits the independent read scale used by 13 sibling Acer models; unit remains unproven |
| RPM | unavailable; no fabricated value |
| TUI | real pty capture of all six original views at 40×20, 80×24, 100×32 and 160×48; faithful PNG render reviewed; installed 2.4.0 pty validation of Dashboard/Profiles/Presets/System against the real supervisor |
| Supervisor IPC | repaired 2.4.0 `neosensed` active and healthy; socket `660 root:neosense`; watchdog timestamp advancing; supervisor reports `NBFC 0.5.4` |
| NBFC socket | `/run/nbfc_service.socket` is `0600 root:root`; ordinary-user connection denied; root supervisor reads live status |
| Thermal policy services | `power-profiles-daemon` active; `fancontrol`, `thinkfan`, `coolercontrol`, `tuned`, `thermald` inactive |

## Host validation status

| Area | Blocker |
|---|---|
| Root systemd unit install/start | **resolved**: repaired 2.4.0 installed 2026-11-11 16:08; installed binaries match artifact `61423d0`; root-protected backup retained |
| Real `neosense` group service session | 2.4.0 supervisor active; non-root CLI/TUI IPC validated under the `neosense` group |
| systemd READY/WATCHDOG on real host | repaired 2.4.0 installed 2026-11-11: `StatusText=NeoSense supervisor ready`, active/running, WatchdogTimestamp advancing, NRestarts=0 |
| ExecStopPost after SIGKILL on real host | not re-run during installation; rollback/recovery path retained in root backup |
| Installer backup/rollback on real host | repaired 2.4.0 installed; `/var/backups/neosense-2.4.0-20261111-142211` and the NBFC backup `nbfc-neosense-20261111-144726` exist (0700 root:root) |
| Non-root read-only control via real host group socket | 2.4.0 CLI/TUI IPC validated under `neosense`; manual control remains blocked |

## Hardware/upstream blocked

| Area | Limitation |
|---|---|
| Protected manual control | NBFC 0.5.4 installed and policy-eligible; blocked by the configuration omission in the read scale, shared `@CPU` mapping and no independent RPM |
| Physical RPM verification | no hwmon tachometer; raw EC values sit in the family tach-like scale but their unit is unproven |
| GPU fan thermal link | NBFC config uses `@CPU` |
| NBFC socket restriction | resolved by the installed patch; only root may connect |
| Closed-loop Quiet/Balanced/Performance and custom curves | curves and persistence implemented; real hardware application not validated |
| Closed-loop hardware control | requires mapping, RPM/independent feedback, backend ownership, recovery and explicit authorization |
| `fan release` | reset-ec/firmware return not validated |
| Physical fan-change tests | require explicit authorization and the independent reference defined in `docs/PHYSICAL_VALIDATION_PROTOCOL.md` (not executed) |

## Isolated validation completed

| Area | Evidence |
|---|---|
| Root install/upgrade/rollback/uninstall | `packaging/test-installer-isolated.sh` in a user namespace; exact-final-archive container lifecycle in `docs/SYSTEMD.md` |
| Release archive contents and prebuilt install | `packaging/test-release-archive.sh` |
| Archive reproducibility | two builds of the same commit produced the same SHA-256 |
| systemd lifecycle | exact final archive, disposable Fedora 44 container; READY, watchdog, socket, stop, SIGKILL, ExecStopPost |
| Group authorization | container: member exit 0, outsider exit 4 |
| Narrow TUI layout | real pty + terminal-emulator capture of all six original views at 40×20, 80×24, 100×32 and 160×48; faithful PNG rendering inspected; System/Diagnostics scroll |

## Not implemented / future work

- Hardware application of closed-loop and custom profiles after independent feedback and recovery validation.
- Real hardware validation of fixed presets and custom curves on a compatible, authorized backend.
- Compatibility database populated by supervised community reports.
- Packaging for `.deb`, `.rpm`, AUR and Nix.
- GitHub release publication (release archive script exists; no API-token release automation).
- Desktop-window screenshot capture in a specific graphical terminal (terminal-cell PNG rendering is already reviewed).
- Full rollback/uninstall exercise on the live host (installation succeeded; rollback retained for a controlled maintenance window).

## Release status

The 2.3.0 release artifact remains frozen and unpublished. The 2.4.0
development line adds the hardware-independent profile simulation engine and
persistent custom curves; it is not a public release and does not enable
hardware closed-loop control.
Publication remains externally blocked; real-host installation/systemd/runtime validation is complete for this machine.
Hardware manual control remains intentionally disabled.

## Next safe actions

1. Keep the root-protected 2.3.0 backup until acceptance. Exercise rollback only in an isolated environment or a controlled maintenance window; do not roll back a working 2.4.0 installation merely to test it.
2. Run read-only `neosense diagnose --json` on additional hardware and record compatibility evidence.
3. Complete Phase 0 of `docs/PHYSICAL_VALIDATION_PROTOCOL.md` (read-only external reference plus raw-read correlation) before considering any write. Do not change the live read scale on the strength of sibling configs alone.
4. Establish independent RPM or a calibrated read range before any manual fan test; `CurrentSpeed=100%` is a clamp artifact, not evidence of fan speed. Phase 1 requires separate explicit authorization.
5. Publish a stable release only after the above evidence is attached to release notes; 2.4.0 remains development-only.
