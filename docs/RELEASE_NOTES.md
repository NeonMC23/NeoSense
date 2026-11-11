# NeoSense 2.3.0 — release notes (prepared)

**Status:** prepared and validated locally; not yet published to GitHub. The
tag and GitHub release should only be created after the remaining real-host
checks in `docs/RELEASE.md` are complete.

## Highlights

- Rust CLI, TUI and single-writer supervisor for NBFC.
- Truthful sensor model: missing, stale and invalid values are never converted
  to zero; EC percentages are never presented as RPM.
- Six TUI views with responsive 40/80/100/160-column layouts.
- Profile compatibility reporting; only `safe-auto` is applicable.
- Group-protected supervisor socket and explicit permission states.
- Categorized diagnostics with JSON summary.
- Shell completions for bash, zsh, fish, elvish and PowerShell.
- Stable JSON schema identifiers and NDJSON `sensors watch` output.
- Scrollable TUI System/Diagnostics views and explicit empty states.
- Hardened installer rollback, bounded IPC reads and safe custom socket paths.
- Snapshot/parser/CLI tests and isolated lifecycle tests.
- Self-installing release archive with prebuilt binaries, checksums and
  deterministic archive metadata.
- Active-supervisor upgrade restart with READY/socket/IPC verification and
  automatic rollback on restart failure; `--no-restart` opts out.
- Cached NBFC version/sensor metadata to reduce redundant polling subprocesses.

## Installation

```bash
sha256sum -c SHA256SUMS
tar -xzf neosense-2.3.0-x86_64-unknown-linux-gnu.tar.gz
cd neosense-2.3.0-x86_64-unknown-linux-gnu
sudo ./install.sh
sudo usermod -aG neosense "$USER"   # then start a new login session
sudo systemctl enable --now neosensed
```

The installer does not start an inactive supervisor. If a supervisor was
already running, it restarts it after upgrade and verifies READY/socket/IPC;
`--no-restart` defers that restart. The restart's designed fail-safe may return
NBFC to automatic mode. `install.sh` uses bundled binaries in the archive; a
source checkout builds with Cargo instead.

## Validation performed

- 69 unit tests, 30 CLI tests, 4 installer tests and 1 language guard (104 total).
- Isolated root install, injected-failure rollback, reinstall and uninstall.
- Release archive contents, checksum, extraction and prebuilt installation.
- Two builds from the same commit produced the same archive SHA-256.
- Disposable Fedora 44/systemd 259 container: READY, socket `0660`, watchdog,
  clean stop, SIGKILL restart, ExecStopPost, group-member access and outsider
  refusal, upgrade backup and uninstall, including active-service restart and
  injected restart-failure recovery.
- Historical 2.2.0 fixture upgraded to 2.3.0 with binary/unit backup integrity,
  legacy state preservation, active-supervisor restart verification and
  injected restart/rollback failure handling.
- Real read-only diagnostics on the Acer Nitro AN515-57 development host.

## Safety limitations

- Manual fan control remains blocked on unvalidated fan mappings/readback.
  Patched NBFC 0.5.4 is installed, but physical identity, RPM feedback and the
  AN515-57 read scale are not validated; explicit hardware authorization is
  required before testing.
- The GPU fan on the development machine is driven from `@CPU`; GPU manual
  control is blocked.
- No physical RPM tachometer is available; physical fan motion is unverified.
- Closed-loop Quiet/Balanced/Performance/Custom profiles are preview-only.
- `fan release` is intentionally unavailable until EC/firmware recovery is
  validated.
- Unpatched NBFC 0.5.3/0.5.4 create their own socket as `0666`. The development
  host runs the reviewed patch (`0600 root:root`, root-only allowlist), so only
  the supervisor can connect there; unpatched systems remain exposed.
- The development host has a validated root installation and non-root
  group-session IPC; other systems still need their own validation.
