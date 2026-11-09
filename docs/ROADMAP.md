# NeoSense — Product roadmap and status

**Updated:** 2026-11-09
**Current version:** 2.3.0

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
| Supervisor polling efficiency | 12s fake-backend measurement: version/config calls reduced 8/8 -> 1/1; `status` unchanged |
| CLI JSON and exit codes | 30 CLI tests; schemas, NDJSON watch and option-rejection coverage |
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

## Validated on the local machine (read-only)

| Area | Result |
|---|---|
| Hardware detection | Acer Nitro AN515-57, i5-11400H, RTX 3050 |
| OS/kernel | Nobara 44, 7.3.0 |
| NBFC | 0.5.3 (`/usr/bin/nbfc`), active, two fans; 0.5.4 is available upstream but not installed |
| CPU/GPU/NVMe/Wi-Fi/ACPI roles | Correct separation |
| GPU fan mapping | Both fans use `@CPU`; GPU manual control blocked |
| EC readback | 100% against varying targets; labelled inconclusive |
| RPM | unavailable; no fabricated value |
| TUI | pty 100×32 smoke, six views |
| Supervisor IPC | current login lacks `neosense`; `sg neosense` reaches the runtime directory; no socket because service is inactive |
| NBFC socket | `/run/nbfc_service.socket` is `0666 root:root`; upstream exposure documented |
| Thermal policy services | `power-profiles-daemon` active; `fancontrol`, `thinkfan`, `coolercontrol`, `tuned`, `thermald` inactive |

## Implemented but awaiting real-system/root validation

| Area | Blocker |
|---|---|
| Root systemd unit install/start | sudo password unavailable in session |
| Real `neosense` group service session | `sg neosense` works; `neosensed` is not running and requires root/systemd |
| systemd READY/WATCHDOG on real host | disposable container passed; real host unit not started |
| ExecStopPost after SIGKILL on real host | disposable container passed; real host unit not run |
| Installer backup/rollback on real host | isolated user-namespace and container installs passed; real host install not run |
| Non-root control via real host group socket | disposable container passed with fake NBFC; real host socket unavailable |

## Hardware/upstream blocked

| Area | Limitation |
|---|---|
| Protected manual control | NBFC 0.5.3; requires >= 0.5.4 |
| Physical RPM verification | no hwmon tachometer |
| GPU fan thermal link | NBFC config uses `@CPU` |
| NBFC socket restriction | upstream hardcodes 0666; no supported override |
| Closed-loop Quiet/Balanced/Performance | no validated curves or hardware evidence |
| `fan release` | reset-ec/firmware return not validated |
| Physical fan-change tests | require explicit authorization |

## Isolated validation completed

| Area | Evidence |
|---|---|
| Root install/upgrade/rollback/uninstall | `packaging/test-installer-isolated.sh` in a user namespace; exact-final-archive container lifecycle in `docs/SYSTEMD.md` |
| Release archive contents and prebuilt install | `packaging/test-release-archive.sh` |
| Archive reproducibility | two builds of the same commit produced the same SHA-256 |
| systemd lifecycle | exact final archive, disposable Fedora 44 container; READY, watchdog, socket, stop, SIGKILL, ExecStopPost |
| Group authorization | container: member exit 0, outsider exit 4 |
| Narrow TUI layout | real pty + terminal-emulator capture of all six views at 40×20, 80×24, 100×32 and 160×48; faithful PNG rendering inspected; System/Diagnostics scroll |

## Not implemented / future work

- Actual closed-loop profile engine with thresholds, hysteresis and transition timing.
- Compatibility database populated by supervised community reports.
- Packaging for `.deb`, `.rpm`, AUR and Nix.
- GitHub release publication (release archive script exists; no API-token release automation).
- Human visual review of TUI screenshots.
- Full root-level end-to-end installation/upgrade/uninstall matrix.

## Next safe actions

1. With sudo: install 2.3.0, add/refresh group session, start `neosensed`, validate READY, socket mode, watchdog, stop and SIGKILL/ExecStopPost.
2. Run read-only `neosense diagnose --json` on additional hardware and record compatibility evidence.
3. Evaluate NBFC 0.5.4 in a disposable environment; it adds JSON status, `reset-ec`, `test-config` and per-fan critical temperatures. Do not install or change configuration on this host without explicit authorization.
4. Obtain explicit authorization and a validated NBFC >= 0.5.4/RPM setup before any manual fan test.
5. Publish a release only after the above evidence is attached to release notes.
