# Real-host installation checklist

This checklist is for a Linux host where NeoSense should become a system
service. It is intentionally separate from container/user-namespace tests.
Nothing here performs a manual fan-speed test.

## Preflight

```bash
command -v nbfc && nbfc --version
systemctl is-active nbfc_service
neosense diagnose --json
sudo -n true
```

- `diagnose` must be read-only and must not report an unknown critical issue.
- If `sudo -n` fails, stop here and obtain an interactive authorization; do not
  use password workarounds.
- Do not continue if NBFC is in manual mode or its service is unhealthy.

## Install a release archive

```bash
sha256sum -c SHA256SUMS
tar -xzf neosense-<version>-<target>.tar.gz
cd neosense-<version>-<target>
sudo ./install.sh
```

The installer:

- refuses to run without root unless `--dry-run` is used;
- creates the `neosense` group if absent;
- backs up existing binaries/unit under
  `/var/lib/neosense/backups/<timestamp>-<pid>/`;
- creates the supervisor-owned state directory
  `/var/lib/neosense` for persistent custom curves;
- installs binaries and the systemd unit;
- verifies both binary versions;
- runs `systemctl daemon-reload` and rolls back if it fails;
- does not start `neosensed` and does not write fan state.

`uninstall.sh` preserves `/var/lib/neosense/profiles.json` (user curves) and
only removes the generated state directory when no profile store exists.

A source checkout uses `sudo ./install.sh` as well; it builds the release
binaries first. A release archive uses its bundled `bin/` binaries and does not
require Rust.

## Apply group membership to the session

```bash
sudo usermod -aG neosense "$USER"
# log out and back in, or in the current shell:
newgrp neosense
```

Verify without starting the service:

```bash
id
ls -ld /run/neosense
neosense service status --json
```

If the current session lacks the supplementary group, the expected state is
`permission_denied` with a group-session hint. `newgrp` only changes the current
shell; a fresh login is the durable fix.

## Start and validate

Only start the supervisor when the operator has authorized a service that can
return NBFC to automatic mode:

```bash
sudo systemctl enable --now neosensed
systemctl is-active neosensed
neosense service status --json
neosense diagnose --json
```

Expected socket state: `/run/neosense/neosensed.sock`, `root:neosense`, mode
`0660`; runtime directory `root:neosense`, mode `0750`; supervisor health
`true`.

Watchdog check: leave the service running longer than `WatchdogSec=10s` and
verify it remains active. Do not use a manual fan-speed test.

Stop and recovery tests can invoke the designed fail-safe path (`nbfc set -a`):

```bash
sudo systemctl stop neosensed
systemctl is-active neosensed || true
sudo systemctl start neosensed
```

A SIGKILL recovery test is deliberately destructive to the service process but
not to hardware; only perform it with authorization:

```bash
sudo systemctl kill -s KILL neosensed
sleep 3
systemctl is-active neosensed
systemctl show neosensed -p NRestarts -p MainPID
```

Inspect the journal for `READY`, `WATCHDOG`, `STOPPING` and
`ExecStopPost` evidence.

## Upgrade

```bash
sudo ./install.sh
neosense version
neosense service status --json
```

If the supervisor was already active, the installer restarts it, waits for
`READY`/socket/IPC, and rolls back and restarts the previous supervisor if the
restart fails. Use `sudo ./install.sh --no-restart` to defer the restart
manually. The shutdown fail-safe may return NBFC to automatic mode during a
restart. The installer creates a new backup before replacing files.

## Rollback an interrupted or bad upgrade

Backups are intentionally left readable by root:

```bash
sudo find /var/lib/neosense/backups -maxdepth 2 -type f
```

Restore the chosen backup manually:

```bash
backup=/var/lib/neosense/backups/<timestamp>-<pid>
sudo install -Dm755 "$backup/bin/neosense" /usr/local/bin/neosense
sudo install -Dm755 "$backup/bin/neosensed" /usr/local/bin/neosensed
sudo install -Dm644 "$backup/systemd/neosensed.service" /etc/systemd/system/neosensed.service
sudo systemctl daemon-reload
sudo systemctl restart neosensed
```

A SIGKILL during installation cannot be trapped by the installer. The backup
directory is the recovery source; verify its files before restoring.

## Uninstall

```bash
sudo ./uninstall.sh
```

The uninstaller stops the supervisor, removes only NeoSense-owned binaries,
unit and runtime state, preserves `profiles.json` (user curves), preserves NBFC
configuration, and keeps the `neosense` group when it still has members. Copy
any backup you want to keep before uninstalling. It does not alter fan settings
outside the supervisor's normal shutdown path.

## Current host status

The development host is an Acer Nitro AN515-57 running Nobara 44, kernel
`7.3.1-202.nobara.fc44.x86_64`.

### NBFC 0.5.4 with root-only access

On 2026-11-11 the patched NBFC 0.5.4 client/service pair was installed from the
reviewed package (SHA-256
`8c25705f5bd5ccc4aa635f998f3915e9b694fc0c9e3edff7f8b49754bfcb7c44`).

- `/etc/nbfc/access.json` is `0644 root:root` with
  `SocketMode 0600`, `SocketOwnerUid 0`, `SocketGroup root`,
  `AllowedClientUids [0]`, `AllowedClientGids []`;
- `/run/nbfc_service.socket` is a Unix socket, `0600 root:root`, no extended
  ACL; an ordinary user connection is denied by the kernel;
- the existing `nbfc_service` unit was preserved; the upstream 0.5.4 unit with
  `ExecStopPost=nbfc reset-ec` was not installed;
- the client/service pair both report `0.5.4`;
- root backup: `/var/backups/nbfc-neosense-20261111-144726`; user-space 0.5.3
  copy: `/tmp/opencode/nbfc-0.5.3-backup-20261111-143558`.

Both NBFC channels remain in automatic mode throughout the integration.

### Why the CLI reported `connect(): Permission denied`

The root supervisor can and does connect to the `0600 root:root` socket: its
reported fan temperature changes on every poll. The misleading
`ERROR: nbfc: connect(): Permission denied` came from `neosense status`,
`neosense diagnose` and `neosense fan status`, which ran `nbfc status` **inside
the ordinary-user CLI process**. Under the old world-writable socket that
worked; under the root-only policy it is an expected denial, not a supervisor
failure.

Two source defects were repaired:

1. `src/runtime.rs` no longer spawns `nbfc`. The CLI/TUI build their
   `RuntimeView` from the supervisor's `status.get` IPC document. Backend
   state, capabilities, sensor sources and the socket inspection are the
   supervisor's; only unprivileged sysfs sensors are collected locally.
   `src/diagnostics.rs` consumes the same view, so diagnostics can no longer
   mistake a user-level denial for a backend outage.
2. The supervisor cached the NBFC client version for its entire lifetime. An
   in-place NBFC upgrade therefore left `protected_manual = false` until the
   supervisor restarted. `Daemon::refresh` now re-probes the version when the
   `nbfc_service` MainPID changes, when the cache ages out (60 s), or when no
   version is known; protected write gates always re-probe the live version
   and fail closed if it is unavailable.

### Installed NeoSense build

The repaired 2.4.0 build was installed on 2026-11-11 at 16:08 and verified
against the repository artifact (commit `61423d0`, SHA-256
`8ca27ead76099b4e8269a7175deb1b22d8a4fdabcd919f6011f80f82bf0a7299`):

- installed binaries are byte-identical to the artifact:
  - `neosense` `7d62336cd086005787eaadc0f667ce3b8ba6754fcc818ed4896abba78e11483a`
  - `neosensed` `346b1b74576f7c1a548c7961de097f2b04b51110b759f349cbb73e476d768e1d`
- the installed unit matches the artifact unit;
- root-protected backup:
  `/var/backups/neosense-2.4.0-20261111-142211`, `0700 root:root`;
- service active/running, `StatusText=NeoSense supervisor ready`,
  `NRestarts=0`, `Result=success`, watchdog timestamp advancing;
- the supervisor now reports `NBFC 0.5.4`, `protected_manual=true`,
  `access_controlled=true`, socket `600 root:root`;
- IPC socket `/run/neosense/neosensed.sock`, `660 root:neosense`;
- StateDirectory `/var/lib/neosense`, `750 root:neosense`; profile store
  `600 root:neosense`; custom profiles empty;
- `neosense diagnose` reports 0 critical issues; backend, service, socket and
  supervisor checks are `ok`;
- `neosense status`, `diagnose`, `fan status` and `hardware identity` still
  work with `PATH=/tmp`, i.e. with no `nbfc` executable available at all.

The CLI/TUI therefore obtain all backend state over IPC; the ordinary user
never connects to the NBFC socket, and the kernel denies `nbfc status` run
directly by that user (expected).

No manual fan target, EC write, NBFC configuration change, BIOS/firmware
change, thermal stress test or unrelated thermal-service change was performed.

Hardware control remains unvalidated. Both logical channels use `@CPU`. The
AN515-57 configuration disables the independent read scale that thirteen
sibling Acer Nitro/Predator configurations declare for the same `19/55` and
`21/58` registers; raw values (`CPU 4225–4615`, `GPU 5769–6122`) are consistent
with that scale, but the unit is not proven. There is no tachometer, and no
physical fan response has been observed. See
[`HARDWARE.md`](HARDWARE.md) and
[`PHYSICAL_VALIDATION_PROTOCOL.md`](PHYSICAL_VALIDATION_PROTOCOL.md).
