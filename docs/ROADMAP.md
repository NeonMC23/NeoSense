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
| TUI rendering and navigation | TestBackend at 40/80/100/160 columns |
| CLI JSON and exit codes | 21 CLI tests |
| Installer dry-runs | Temporary prefix/systemd/runtime/state |
| Isolated root install/rollback/uninstall | `packaging/test-installer-isolated.sh` passed in user namespace |
| Release build | `cargo build --release --locked` |
| Release archive | `packaging/build-release.sh` generated tarball and checksums |
| Prebuilt archive install | `packaging/test-release-archive.sh` extracts and installs without Cargo |
| systemd unit in disposable container | Fedora 44/systemd 259: READY, socket `0660`, watchdog, clean stop, SIGKILL recovery, ExecStopPost |
| Non-root IPC in disposable container | `neosense` group member succeeded; outsider exited 4 (fake NBFC backend) |

## Validated on the local machine (read-only)

| Area | Result |
|---|---|
| Hardware detection | Acer Nitro AN515-57, i5-11400H, RTX 3050 |
| OS/kernel | Nobara 44, 7.3.0 |
| NBFC | 0.5.3, active, two fans |
| CPU/GPU/NVMe/Wi-Fi/ACPI roles | Correct separation |
| GPU fan mapping | `@CPU`; GPU manual control blocked |
| EC readback | 100% against varying targets; labelled inconclusive |
| RPM | unavailable; no fabricated value |
| TUI | pty 100×32 smoke, six views |
| Supervisor IPC | permission-denied state correctly classified |

## Implemented but awaiting real-system/root validation

| Area | Blocker |
|---|---|
| Root systemd unit install/start | sudo password unavailable in session |
| Real `neosense` group session | new login session required; group membership exists |
| systemd READY/WATCHDOG on real host | disposable container passed; real host unit not started |
| ExecStopPost after SIGKILL on real host | disposable container passed; real host unit not run |
| Installer backup/rollback on real host | isolated user-namespace and container installs passed; real host install not run |
| Non-root control via real host group socket | disposable container passed with fake NBFC; real host socket unavailable |
| Release archive generation | deterministic tarball, completions, BUILD-INFO and SHA256SUMS generated locally; prebuilt archive install validated |

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
| Root install/upgrade/rollback/uninstall | `packaging/test-installer-isolated.sh` in a user namespace |
| Release archive contents and prebuilt install | `packaging/test-release-archive.sh` |
| Archive reproducibility | two builds of the same commit produced the same SHA-256 |
| systemd lifecycle | disposable Fedora 44 container; READY, watchdog, socket, stop, SIGKILL, ExecStopPost |
| Group authorization | container: member exit 0, outsider exit 4 |
| Narrow TUI layout | rendered inspection at 40×20, 80×24 and 100×32 |

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
3. Obtain explicit authorization and a validated NBFC >= 0.5.4/RPM setup before any manual fan test.
4. Publish a release only after the above evidence is attached to release notes.
