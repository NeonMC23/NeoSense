# NeoSense — systemd, group and watchdog

## Group and IPC

The installer creates a system group `neosense` if absent. It does not change user memberships.

```bash
sudo usermod -aG neosense "$USER"
# Log out and back in, or: newgrp neosense
```

Socket: `/run/neosense/neosensed.sock`, `root:neosense`, `0660`. Runtime directory: `root:neosense`, `0750`. The supervisor applies permissions explicitly before `READY=1` and fails startup if the group is missing or ownership cannot be applied.

**Upstream limitation:** NBFC's own socket is `0666`. The `neosense` group protects NeoSense only; any local user can still call NBFC directly. Do not modify NBFC permissions without upstream/hardware validation.

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

Not performed: root unit installation, real `neosense` group membership test, real NBFC `set -a` on hardware.
