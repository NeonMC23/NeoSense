# NBFC access-control proposal

Status: technical proposal only. Not submitted upstream and not installed on the
host.

## Problem

NBFC-Linux creates its control socket in `src/server.c` with:

```c
bind(...);
chmod(NBFC_SOCKET_PATH, 0666);
```

The shipped systemd unit does not set socket ownership, mode or socket
activation. The upstream README states **Fan Control Rights: Any user**, so any
local process can send `set-fan-speed` and bypass a supervisor such as NeoSense.
This is present in 0.5.3, 0.5.4 and the inspected master commit `880087d`.

Setting the socket to `0660 root:neosense` is not sufficient when the
`neosense` group contains ordinary CLI users: those users can connect directly
to NBFC and bypass supervisor policy. NeoSense therefore rejects `0660
root:neosense` unless the group is verified to contain only the supervisor.

## Proposed minimal upstream change

Two independently useful controls:

### 1. Configurable socket ownership and mode

Add service configuration fields, for example:

```json
{
  "SocketMode": "0600",
  "SocketGroup": "root"
}
```

or equivalent command-line options. During `Server_Init`:

1. create and bind the socket;
2. resolve the configured group;
3. `chown(NBFC_SOCKET_PATH, 0, configured_gid)`;
4. `chmod(NBFC_SOCKET_PATH, configured_mode)`.

The operation must be applied after every bind, so restart and reboot are
covered. Package upgrades must preserve the configuration file. Invalid mode
or unresolvable group must abort startup rather than silently falling back to
`0666`.

Recommended secure default for supervisor deployments: `0600 root:root`.
For package compatibility, a transitional default may remain `0666`, but
NeoSense must then continue to refuse manual hardware control.

### 2. Peer-credential authorization on control commands

In `Server_Command_Set_Fan` and any future reset/EC command, obtain the peer
credentials with `SO_PEERCRED`:

```c
struct ucred cred;
socklen_t len = sizeof(cred);
getsockopt(socket, SOL_SOCKET, SO_PEERCRED, &cred, &len);
```

Allow only configured UIDs or members of a configured dedicated group.
Suggested policy:

- `status` may remain readable to all local users;
- `set-fan-speed`, `reset-ec` and other control commands require root or an
  explicitly configured authorized UID/group;
- reject by default if the peer identity cannot be obtained;
- do not infer authorization from socket mode alone.

Current master uses `SO_PEERCRED` only for suspend/resume, not for fan commands.

## Why not implement a NeoSense proxy now?

A privileged proxy would be a second root/near-root service with its own
protocol, lifecycle, restart and authentication surface. It cannot make an
unrestricted NBFC socket safe for other clients; those clients can still connect
to NBFC directly. It is justified only if NBFC cannot be changed **and** the
proxy can stop other clients from reaching NBFC, which requires the same access
control change. It is not a substitute for upstream enforcement.

## Validation required for the upstream change

- Fresh install, upgrade, restart and reboot preserve the configured mode and
  ownership.
- `0666` is impossible under a secure configuration.
- `0660` with a dedicated group containing only the supervisor permits the
  supervisor and rejects an ordinary user.
- `0600 root:root` rejects an ordinary user and permits root.
- Missing peer credentials fail closed.
- `status` remains available read-only if intended.
- Existing NBFC clients and package service lifecycle are tested.
- NeoSense's capability gate can then be re-evaluated against the real backend.

## Isolated patch prepared for review

Bases verified independently:

- upstream tag `0.5.4`, commit `9da010d6a2e8351bdf6900d6c3643f1e742af936`;
- current upstream master `880087d` (which adds suspend/resume).

Patch locations (outside this repository):

- `/tmp/opencode/nbfc-access-control-0.5.4.patch`
  (SHA-256 `9d0eee69a5127bbd65a907dcaf2ce10a34fc9c50171a1e4a98ac2d95a24f4073`)
- `/tmp/opencode/nbfc-access-control-master.patch`
  (SHA-256 `d1fe347e182a878b1d29e70d9b5647e2d8f27ec2e784edcf2b3acd43888a275b`)

Temporary source trees: `/tmp/opencode/nbfc-patch` (0.5.4) and
`/tmp/opencode/nbfc-patch-master` (master). Both build with
`make src/nbfc_service`; both pass the isolated access-control test.

Changes:

- `src/access_control.c`, `src/access_control.h`: strict JSON loader, octal
  mode parser, group resolution, owner/mode application and verification,
  peer-credential authorization with UID/GID allowlists and supplementary-group
  resolution.
- `src/server.c`: loads the access configuration before socket creation;
  creates the socket under `umask(0077)`; applies and verifies the configured
  ownership/mode before `listen()`; authorizes `set-fan-speed` before dispatch.
  The master port also authorizes suspend/resume before dispatch.
- `src/nbfc.h`: default `/etc/nbfc/access.json` path, overridable with
  `NBFC_ACCESS_CONFIG`.
- `src/build.c`, `Makefile.in`: build integration.
- `tests/access_control_test.c`, `tests/run-access-control.sh`: isolated
  parser, ownership, peer-credential and denial tests.

Build result: `make src/nbfc_service` succeeds. Isolated test result:
`access-control tests: OK`.

Missing access configuration uses the secure defaults
`0600 root:root`, root-only control. Malformed configuration aborts startup.
Root remains inside the administrative trust boundary.

## NeoSense integration contract

For the current root supervisor and the ordinary CLI user in `neosense`, the
recommended NBFC configuration is:

```json
{
  "SocketMode": "0600",
  "SocketOwnerUid": 0,
  "SocketGroup": "root",
  "AllowedClientUids": [0],
  "AllowedClientGids": []
}
```

Expected result:

- NeoSense supervisor (UID 0) can connect and control;
- ordinary CLI/TUI users cannot connect to NBFC directly;
- `nbfc status` as an ordinary user fails at connect time;
- root/admin processes remain inside the trust boundary;
- no `neosense` group access is granted;
- NeoSense classifies `0600 root:root` with a root supervisor as
  access-controlled, while `0660 root:neosense` remains rejected.

The configuration is read on every NBFC service start, so it survives restart
and reboot. Packaging must preserve `/etc/nbfc/access.json` across upgrades.
Malformed configuration must fail startup rather than fall back to `0666`.

Verification after deployment:

```bash
stat -c '%a %U:%G' /run/nbfc_service.socket
# expect 600 root:root

nbfc status              # as ordinary user: connection denied
sudo nbfc status         # as root: works
```

No privileged proxy is required or recommended. The patch keeps NBFC as the
device-control backend and NeoSense as the policy authority.
