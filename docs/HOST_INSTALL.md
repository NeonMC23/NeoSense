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
unit, runtime and state (including its backups), preserves NBFC configuration,
and keeps the `neosense` group when it still has members. Copy any backup you
want to keep before uninstalling. It does not alter fan settings outside the
supervisor's normal shutdown path.

## Current host status

The development host used for this checklist is an Acer Nitro AN515-57 running
Nobara 44, kernel 7.3.0, NBFC 0.5.3. At the time of writing:

- `sudo -n true` requires a password, so the real-host install was not run;
- `getent group neosense` contains `neon`, but the current login session does
  not include the supplementary group until `newgrp` or a new login;
- `/run/neosense` is `root:neosense`, mode `0750`;
- a previously installed 2.3.0 `neosensed` was observed active and healthy
  under `sg neosense`; `/var/lib/neosense` exists;
- the 2.4.0 development build is not installed on the host;
- read-only diagnostics report `operational: true`, 0 critical issues, and
  warnings for NBFC 0.5.3 limitations, EC readback, GPU mapping and supervisor
  access.

This is an audit record, not a claim that the real-host installation is
complete.
