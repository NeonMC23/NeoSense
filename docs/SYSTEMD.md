# NeoSense — systemd, group and watchdog

## Group and IPC

The installer creates a system group `neosense` if absent. It does not change user memberships.

```bash
sudo usermod -aG neosense "$USER"
# Log out and back in, or: newgrp neosense
```

Socket: `/run/neosense/neosensed.sock`, `root:neosense`, `0660`. Runtime directory: `root:neosense`, `0750`. The supervisor applies permissions explicitly before `READY=1` and fails startup if the group is missing or ownership cannot be applied.

**Upstream limitation:** NBFC 0.5.3 and 0.5.4 create `/run/nbfc_service.socket` as `root:root` and hardcode `chmod 0666` in `server.c`; no supported socket-permission configuration was found. The `neosense` group protects NeoSense only; any local user can still call NBFC directly. No permission override or NBFC configuration change was applied.

## Unit

`packaging/neosensed.service` installed as `/etc/systemd/system/neosensed.service`.

- `Type=notify`, `NotifyAccess=main`, `WatchdogSec=10s`.
- `TimeoutStartSec=30s`, `TimeoutStopSec=10s`.
- `Restart=on-failure`, `RestartSec=2s`, start limits.
- `RuntimeDirectory=neosense`, mode `0750`, no preserve.
- `User=root`, `Group=neosense`, `UMask=0007`.
- `NEOSENSE_SOCKET_GROUP=neosense`, explicit socket path.
- `ExecStopPost=neosensed --fail-safe`.
- Hardening: `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `ProtectKernelTunables`, `ProtectKernelModules`, `ProtectControlGroups`, `RestrictAddressFamilies=AF_UNIX AF_NETLINK`, `LockPersonality`, `SystemCallArchitectures=native`.

No `PrivateDevices`, capability-narrowing or syscall filter is enabled without hardware validation.

## Watchdog

- `READY=1` only after socket bind/permissions and an initial consistent snapshot.
- `WATCHDOG=1` only while health is healthy: fresh sensor sample, consistent state, coherent manual lease and NBFC available during manual control.
- NBFC outage in automatic mode is degraded but does not force writes or heartbeat failure; restarting would not repair NBFC.
- `STOPPING=1` on orderly shutdown.

The watchdog does not guarantee physical fan motion, EC correctness or sensor accuracy.

## Recovery

| Event | Behaviour |
|---|---|
| SIGTERM/stop | STOPPING, automatic recovery, socket removal, ExecStopPost |
| Init failure | no READY; startup fails |
| SIGKILL/OOM | no Rust handler; ExecStopPost |
| Watchdog expiry | systemd kills/restarts; ExecStopPost |
| Repeated restart | bounded by start limits |

`neosensed --fail-safe` calls `nbfc set -a`, re-reads status, verifies all fans report automatic mode, and exits non-zero otherwise. It is idempotent, does not call `reset-ec`, and does not modify NBFC config.

## Validation performed

With a fake `nbfc` in `PATH` (no hardware writes) and a temporary socket:

- user `systemd-run` unit with `Type=notify`/`WatchdogSec=10s`: active after 11 s, READY recognised;
- clean stop;
- SIGKILL of `MainPID` followed by systemd restart with a new PID;
- `ExecStopPost` wrapper invoked; fake `set -a` and verified automatic state;
- `neosense fan auto` without sudo through the temporary socket.

A disposable rootless Podman container (Fedora 44, systemd 259, privileged
container namespace) was also used to validate the installed unit and archive:

- archive installed with `install.sh` as container root, no Cargo required;
- `systemctl enable --now neosensed` reached `active (running)` with `READY`;
- socket `/run/neosense/neosensed.sock` was `root:neosense`, mode `0660`;
  runtime directory was `root:neosense`, mode `0750`;
- `WatchdogUSec=10s` remained active past the watchdog interval;
- a non-root user in `neosense` could call `neosense service status` and
  `fan auto` over IPC; an outsider received exit 4;
- clean stop removed the runtime directory and invoked the fail-safe path;
- `SIGKILL` of `MainPID` caused a restart (`NRestarts=1`) and a new PID;
- `neosense diagnose --json` was consulted as a read-only check;
- a second archive install performed an in-place upgrade with a backup and
  restarted the unit on the new binary;
- `uninstall.sh` stopped the unit, removed only NeoSense-owned binaries, unit,
  runtime and state, and preserved the unrelated fake NBFC and group membership.

The container used a fake `nbfc` backend and did not write real hardware. Not
performed: root unit installation on the real host and real NBFC `set -a` on
hardware.

## Exact final archive lifecycle (2026-11-09)

The exact release archive built from the current release code was checked
against `dist/SHA256SUMS` before the run. Its exact checksum and source commit
are recorded in the external release artifact/BUILD-INFO and in the validation
report; they are not embedded in the archive itself to avoid a self-referential
checksum.

It was installed in a disposable privileged rootless Podman container
(Fedora 44, systemd 259) with a fake `nbfc` backend and a fake
`nbfc_service.service`. The following were executed:

- fresh archive install as container root;
- `systemctl enable --now neosensed` with READY/active state;
- socket `root:neosense`, mode `0660`; runtime directory `0750`;
- group member `neosense service status` succeeded; outsider exited 4;
- `fan auto` as a group member succeeded through the supervisor;
- `fan set` exited 8 and the fake backend log contained no `set -f` write;
- watchdog stayed active beyond `WatchdogUSec=10s`;
- clean stop removed the runtime directory and invoked the fail-safe path;
- SIGKILL of `MainPID` produced a new PID and `NRestarts=1`;
- reinstall of the same archive created a second backup and preserved the
  installed hashes;
- an injected `systemctl daemon-reload` failure rolled binaries and unit back
  to their pre-install hashes while the service stayed active;
- uninstall stopped the unit and removed only NeoSense-owned binaries, unit,
  runtime and state; the fake NBFC binary, `/etc/nbfc/unrelated.conf` and
  `/usr/share/nbfc/configs/unrelated.json` remained; the group remained
  because a member existed.

The upgrade case was an in-place reinstall of the same 2.3.0 archive, not a
cross-version upgrade. It validates backup creation and replacement, but a
future versioned upgrade still needs its own real-host/isolated test.
