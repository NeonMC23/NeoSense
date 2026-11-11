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

## Supported versions

Security fixes target the current main branch and the latest release. Please
include the NeoSense version, NBFC version, distribution, kernel and relevant
diagnostic output in reports.
