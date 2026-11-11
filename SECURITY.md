# Security policy

## Reporting

Please report suspected vulnerabilities privately through GitHub Security
Advisories for `NeonMC23/NeoSense`. Do not open a public issue for an exploitable
privilege or hardware-control vulnerability.

## Threat model and known limitations

NeoSense is a local Linux utility. It is not a sandbox or a complete security
boundary for NBFC.

- The NeoSense supervisor socket is created as `root:neosense`, mode `0660`.
  Authorized group members can request control operations.
- **NBFC 0.5.3 creates `/run/nbfc_service.socket` as `root:root` with hardcoded
  mode `0666`.** Any local user can call NBFC directly, bypassing NeoSense.
  NBFC 0.5.4 and the inspected upstream master still contain the same hardcoded
  `chmod(NBFC_SOCKET_PATH, 0666)` in `src/server.c`; there is no supported
  socket-permission option.
- NeoSense detects group/world-writable NBFC sockets, reports them as a
  security warning, and refuses manual/preset/closed-loop hardware operations
  while the backend is not access-controlled. Automatic `fan auto` remains
  available because it is the recovery operation. No periodic `chmod`, socket
  deletion or permission race is used.
- A robust fix requires an upstream NBFC change (configurable socket ownership
  and mode, or per-command peer-credential policy) or another verified
  access-control mechanism. The current `0666` socket means NeoSense cannot
  claim an exclusive fan-control boundary.
- NeoSense does not request root in the CLI for read-only operations. Control
  requires supervisor access or explicit privilege.
- `sudo`/root access is required to install the systemd unit and manage the
  `neosense` group.
- The application stores no credentials and makes no network requests.

## NBFC socket access classification

Permission bits alone are not sufficient. NeoSense evaluates:

- existence and Unix-socket type;
- owner UID against the verified supervisor UID;
- mode bits, including owner/group/other write;
- the socket group's local members and primary-GID users;
- POSIX ACL presence, which fails closed;
- whether the supervisor itself can connect;
- whether the supervisor identity is verified.

The resulting rules are:

| Case | Classification |
|---|---|
| `0600 root:root`, supervisor UID 0 | suitable; root-equivalent processes remain inside the trusted boundary |
| `0600 root:root`, supervisor non-root | rejected; owner mismatch and supervisor cannot connect |
| `0600` owned by the non-root supervisor | suitable only if identity and group data are verified |
| `0660 root:neosense` with ordinary `neosense` users | **rejected**; those users can bypass the supervisor |
| `0660 root:<dedicated group>` with only root/supervisor | accepted if local group membership is fully resolved |
| `0660` with an unexpected or unresolved group | rejected |
| `0666` | always rejected |
| missing socket, non-socket path, ACL, unverified identity | rejected |

Root is treated as inside the administrative trust boundary: another root
process can always bypass a `0600` socket. NeoSense therefore claims protection
against unprivileged local users, not against root or a compromised supervisor.

The supervisor re-inspects the socket immediately before manual/preset
operations and invalidates capability state when permissions change. A manual
or preset lease is recovered to automatic/fail-safe when access control is lost.

CLI/TUI diagnostics use the NeoSense IPC socket owner to infer the supervisor
UID when the command is not run as root. If that cannot be established, the
socket is reported as unverified rather than secure.

## Isolation options

| Option | Possible | Upstream-supported | Durability | New privileged surface | Preserves supervisor authority | Assessment |
|---|---|---|---|---|---|---|
| Configurable NBFC socket owner/mode | Yes | No | Yes, if packaged in unit/sysconfig | Low | Yes | Preferred upstream fix: default `0600 root:root` or a dedicated group |
| `SO_PEERCRED` authorization on fan commands | Yes | No; master only protects suspend/resume | Yes | Low | Yes, NBFC enforces the client identity | Strongest upstream fix, but would need a defined authorized UID/group |
| Dedicated OS group with `0660` | Only if NBFC can be made to use it | No, hardcoded `0666`/root | Only with upstream/package support | Low-medium | Yes if group membership is tight | Not sufficient while the group is `neosense` and contains CLI users |
| systemd socket activation/permissions | Theoretically | No; NBFC creates the socket itself | Yes | Low | Yes | Requires NBFC to accept an inherited socket; not available |
| Separate privileged proxy | Yes | No | Yes if managed carefully | **High**: another root/near-root daemon and protocol | Only if the proxy has no other clients | Not justified before an upstream fix; larger attack surface |
| Alternative backend | Not on this host | N/A | N/A | N/A | Yes | No hwmon/PWM/acer_wmi fan path; future hardware-dependent |

The safest practical path is an upstream NBFC change that makes socket ownership
and mode configurable and/or enforces `SO_PEERCRED` on fan-control commands.
NeoSense should not add a privileged proxy merely to compensate for an upstream
socket that remains available to every local user.

## Supported versions

Security fixes target the current main branch and the latest release. Please
include the NeoSense version, NBFC version, distribution, kernel and relevant
diagnostic output in reports.
