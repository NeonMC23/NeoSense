# NeoSense Linux installation

NeoSense ships one unified entry point that detects the host, checks the
toolchain and runtime, prepares the fan backend when possible, installs the
software, and verifies the result.

```bash
sudo ./neosense.sh
```

The script locates its own resources and can be run from the repository root,
an extracted release archive, another mounted partition, or any current working
directory. It always changes the currently running Linux installation.

`install.sh` remains the low-level transactional file installer. `neosense.sh`
is the supported user entry point; it delegates binary/unit installation and
rollback to `install.sh` instead of maintaining a second installation state.

> **Trust boundary:** run the installer only from a trusted checkout or a
> checksum-verified release archive. It sources shell modules and executes
> package-manager operations as root. NeoSense never downloads or runs an
> unverified upstream installer.

## What the installer does

1. Reads `/etc/os-release`, architecture, kernel, systemd and package manager.
2. Reports the detected distribution and hardware model.
3. Checks build, runtime and optional dependencies with version constraints.
4. Resolves missing packages through DNF, APT, Pacman or Zypper after consent.
5. Detects the NBFC backend and competing fan controllers.
6. Refuses to claim fan control when no supported backend is operational.
7. Builds from source or uses prebuilt release binaries.
8. Installs binaries, the systemd unit, group, socket and state directory.
9. Enables and starts the supervisor only when policy and backend allow it.
10. Runs a read-only verification and reports an explicit capability status.

No fan target is written during installation, verification or diagnostics.
NBFC configuration is never replaced.

## Commands

| Command | Purpose |
|---|---|
| `sudo ./neosense.sh install` | Full detection, dependency preparation, installation and verification (default) |
| `sudo ./neosense.sh upgrade` | Same as install; preserves profiles and validation evidence |
| `sudo ./neosense.sh repair` | Re-run the software installation and verification |
| `./neosense.sh verify` | Read-only installation, backend and capability verification |
| `./neosense.sh diagnose` | Alias of `verify` |
| `./neosense.sh deps` | Dependency matrix and current status |
| `./neosense.sh detect` | Distribution, architecture, systemd and hardware detection |
| `sudo ./neosense.sh uninstall` | Remove NeoSense software; preserve profiles, validation evidence and NBFC |

There is no separate `build` command. `install`, `upgrade` and `repair` build
from source when no prebuilt `bin/` directory is present or when `--source` is
given. `--prebuilt` requires the archive's binaries.

Common options:

```text
--yes                 Authorize listed package and software installation
--dry-run             Print privileged actions without performing them
--no-restart          Leave an already-running supervisor untouched
--no-start            Do not enable or start the supervisor
--skip-deps           Do not install missing packages (still validates)
--software-only       Install software without a ready fan backend
--install-backend     Allow a distro NBFC package installation
--backend-package=N   Install a specific NBFC package
--prebuilt / --source Force the installation path
```

Non-interactive execution must use `--yes`; it never silently installs packages
or starts services. `--yes` does not authorize backend replacement: a backend
package is installed only with `--install-backend` or `--backend-package`.

## Distribution support

| Distribution | Detection and package adapter | Installer flow | Hardware validation |
|---|---|---|---|
| Nobara 44 / Fedora | Implemented (DNF) | Software-only install, upgrade, rollback and uninstall validated in a Fedora 44 KVM VM; Nobara host installation present | Acer Nitro AN515-57 is the primary tested machine; manual control remains policy-gated |
| Debian / Ubuntu | Implemented (APT) | Mock-tested | No hardware validation on this project |
| Arch Linux and derivatives | Implemented (Pacman) | Mock-tested | No hardware validation |
| openSUSE | Implemented (Zypper) | Mock-tested | No hardware validation |
| Other distributions | Detection only | Refused for installation; read-only diagnostics still work | None |

“Mock-tested” means the detection, dependency and package-adapter paths were
exercised with isolated fixtures and fake package managers. It does not mean
that a complete installation was performed on that distribution or laptop.

Architectures: `x86_64` is the primary tested target. `aarch64` is accepted for
source builds but has no hardware validation. Other architectures are refused.

## Dependency matrix

### Build dependencies (source install only)

| Dependency | Minimum | Purpose | Fedora | Debian/Ubuntu | Arch | openSUSE |
|---|---|---|---|---|---|---|
| `rustc` | 1.85 | Rust compiler | `rust` | `rustc` | `rust` | `rust` |
| `cargo` | 1.85 | Locked release build | `cargo` | `cargo` | `rust` | `cargo` |
| C compiler (`cc`) | any | Linker driver | `gcc` | `build-essential` | `gcc` | `gcc` |
| `ld` | any | Linker | `binutils` | `binutils` | `binutils` | `binutils` |

A prebuilt release archive does not need Rust or a compiler.

For a Rust toolchain older than 1.85, the installer reports the requirement and
does not download an arbitrary script. Install rustup manually from
<https://rustup.rs> or use a prebuilt archive.

### Runtime and installation dependencies

| Dependency | Minimum | Purpose | Fedora | Debian/Ubuntu | Arch | openSUSE |
|---|---|---|---|---|---|---|
| `bash` | 4.0 | Installer scripting and arrays | `bash` | `bash` | `bash` | `bash` |
| `systemd` | 245 | Unit, watchdog, sandboxing | `systemd` | `systemd` | `systemd` | `systemd` |
| `grep` | any | Status and configuration parsing | `grep` | `grep` | `grep` | `grep` |
| coreutils | any | install, stat, sha256sum, mktemp | `coreutils` | `coreutils` | `coreutils` | `coreutils` |
| `tar` | any | Archive extraction and backups | `tar` | `tar` | `tar` | `tar` |
| `gzip` | any | Compressed archives/backups | `gzip` | `gzip` | `gzip` | `gzip` |
| `sed` | any | Unit path substitution | `sed` | `sed` | `sed` | `sed` |
| `awk`/`gawk` | any | Status and version parsing | `gawk` | `gawk` | `gawk` | `gawk` |
| `getent` | any | Group resolution | `glibc-common` | `libc-bin` | `glibc` | `glibc` |
| `groupadd` | any | Service group creation | `shadow-utils` | `passwd` | `shadow` | `shadow` |
| NBFC | 0.5.4 recommended | Fan backend | distro package or verified build | distro package or verified build | distro package or verified build | distro package or verified build |

### Optional dependencies

| Dependency | Purpose | Installation |
|---|---|---|
| `nvidia-smi` | Independent NVIDIA GPU temperature guard on hybrid laptops | Provided by the NVIDIA driver; never auto-installed |
| `acpiexec` | Optional NBFC `rate-config` analysis | `acpica-tools` / `acpica` |
| `shellcheck` | Development only | Not required at runtime |

## Fan-control backend

The installer detects NBFC through `nbfc --version`, `nbfc_service`, the NBFC
socket and `/etc/nbfc/nbfc.json`. It reports one of:

| State | Meaning |
|---|---|
| `ready` | NBFC 0.5.4+ is active with a socket and configuration |
| `read_only` | NBFC 0.5.3 is present; automatic mode is available, protected manual control is not |
| `missing` | No NBFC executable was found |
| `unavailable` | NBFC exists but its service or socket is not available |
| `incompatible` | Version or reported metadata is below the supported baseline |
| `conflict` | Another fan controller (`fancontrol`, `thinkfan`, `coolercontrol`) is active |

The installer never stops another controller and never changes NBFC
configuration. If NBFC is missing it explains the distribution package option
and the verified upstream requirement. A distro package is installed only with
`--install-backend` (or an explicit `--backend-package`), after confirmation.
Configuration under `/etc/nbfc` is backed up before that package operation; the
distribution package manager may enable or start its own NBFC unit, and the
installer does not add a second start/stop. Existing configuration is not
rewritten by NeoSense.

NBFC 0.5.4 or newer is required for protected manual control. NBFC 0.5.3 remains
usable for read-only diagnostics and automatic mode. A missing or unusable
backend aborts the default installation before NeoSense files are changed; use
`--software-only` to install the application without claiming fan control.

**Acquisition status (2026-11-18):** on the target Nobara 44 host, `dnf info
nbfc` and `dnf info nbfc-linux` reported no available package. The upstream
`nbfc-linux` project publishes 0.5.4 distribution packages (Fedora 43/44,
Debian, Ubuntu, Arch, openSUSE) with SHA-256 checksums on its official release
page, but the NeoSense installer deliberately does not download or install them
automatically. Automatic acquisition therefore remains unresolved; the
installer only offers a distro package with explicit consent and otherwise
stops with instructions. It never downloads an arbitrary upstream script.

### Manual verified backend path

The official `nbfc-linux` 0.5.4 release page is
<https://github.com/nbfc-linux/nbfc-linux/releases/tag/0.5.4>. For the target
Fedora 44/Nobara 44 family, the published asset is
`fedora-44-nbfc-linux-0.5.4-1.x86_64.rpm` with SHA-256
`69689a3c2a4523119c7488a2f72a6133d3d9a530a982cd19c42c4b4d42253617`.
Other distribution assets and hashes are published on the same page.

A cautious manual path is:

```bash
# Download the asset from the release URL, then verify the published checksum
printf '%s  %s\n' \
  '69689a3c2a4523119c7488a2f72a6133d3d9a530a982cd19c42c4b4d42253617' \
  'fedora-44-nbfc-linux-0.5.4-1.x86_64.rpm' | sha256sum -c -
# Review the package and preserve any existing /etc/nbfc before installing
sudo dnf install ./fedora-44-nbfc-linux-0.5.4-1.x86_64.rpm
```

This path was exercised only inside the disposable Fedora 44 VM. The package
installed, but `nbfc_service` could not start without `/etc/nbfc/nbfc.json` and
compatible EC hardware, and no socket was created. The install was not treated
as backend-ready.

Automatic acquisition remains deliberately deferred. The official RPM/DEB
assets are checksum-published but not signed, and package installation still
cannot choose a safe laptop profile or guarantee compatible EC hardware.
`--install-backend` with an explicit distro package remains the conservative
path; the installer never downloads or replaces a backend silently.

### Additional backends

`fancontrol`/hwmon and `thinkfan` are **not** implemented as NeoSense control
backends. They are detected as competing services only. The Rust supervisor has
a `FanBackend` trait as the extension point; adding another backend requires a
real adapter, capability verification and conflict handling. Do not run
multiple controllers against the same physical fan.

## Safety and recovery

- Installation, verification and diagnostics never write a fan target.
- NBFC configuration and competing services are not modified automatically.
- Existing profiles and validation evidence are preserved on upgrade and
  uninstall.
- `install.sh` creates a timestamped backup and restores previous binaries/unit
  if installation verification fails.
- Package installation failure leaves the dependency unresolved; the installer
  exits non-zero and does not claim success.
- A missing backend stops the default installation before NeoSense software is
  changed. `--software-only` is explicit and does not start the supervisor.
- An interrupted `install.sh` triggers its rollback trap. Package-manager
  transactions are owned by the distribution package manager.
- The running supervisor's designed fail-safe may restore NBFC automatic mode
  when it stops or is restarted. No new fan target is issued by the installer.
- Uninstall stops the NeoSense supervisor, preserves custom profiles and
  validation evidence, and does not touch NBFC.

## Verification statuses

`./neosense.sh verify` prints one of:

| Status | Meaning |
|---|---|
| `READY` | Software, backend, service and physical validation all confirmed |
| `READY_WITH_LIMITATIONS` | Software and backend operational; operator-attested actuator response may be recorded, but trusted independent RPM/airflow calibration and physical-mapping evidence are unavailable |
| `BACKEND_UNAVAILABLE` | NBFC is missing, inactive or unusable |
| `CONTROL_NOT_ELIGIBLE` | Policy or a competing controller blocks manual control |
| `INSTALLATION_INCOMPLETE` | Binaries, unit, version or IPC checks failed |

The current project cannot honestly report `READY` on the Acer laptop because
trusted independent RPM/airflow calibration and physical fan-mapping evidence
remain unavailable. Operator-attested CPU→LEFT and GPU→RIGHT actuator response is
recorded in `docs/VALIDATION_RESULTS.md`. Expect `READY_WITH_LIMITATIONS` when
everything software-side is correct; this does not mean the physical response
was never observed.

Installer/verification exit codes:

| Code | Meaning |
|---:|---|
| 0 | Ready |
| 1 | Installation incomplete or an unresolvable dependency/backend action failed |
| 2 | Usage error |
| 6 | Software installed; hardware or physical-feedback validation remains incomplete |
| 7 | Backend missing, unavailable or incompatible |
| 8 | Control not eligible (policy or competing controller) |

Exit code 6 is expected on the current Acer installation and is not a software
installation failure.

## Target Acer Nitro 5 AN515-57 assessment

- Detection: Acer Nitro AN515-57, `Scala_TLS`, BIOS V1.17, Nobara 44, x86_64,
  systemd 259, DNF.
- Installed NBFC: patched 0.5.4, socket `0600 root:root`, active service.
- Backend state: `ready`.
- Software verification: binaries and unit present, supervisor active, IPC
  reachable.
- Manual control: policy-eligible on this installation. Operator-attested
  CPU→LEFT and GPU→RIGHT actuator response is recorded.
- Independent RPM/airflow and physical mapping instrumentation: unavailable.
  EC tach words are provisional and never treated as RPM.
- The installer was **not** re-run against production during this work; the
  live installation was left untouched.

## Diagnostics

```bash
./neosense.sh detect
./neosense.sh deps
./neosense.sh verify
neosense status
neosense diagnose
neosense control readiness --json
neosense service status
```

`neosense diagnose --json` remains the machine-readable source for control and
calibration readiness.

## Isolated systemd validation (2026-11-18)

A clean software installation was performed in a disposable Fedora 44
systemd container built from the official Fedora image, using the extracted
release archive. The container used an independent root filesystem and
systemd service manager; it shared the host kernel and `/sys`.

Results:

- `./neosense.sh install --yes --software-only --no-start` installed both
  binaries, the unit, group, runtime and state directories.
- `systemctl` recognized the unit; binary versions and ownership were correct.
- In a privileged rootless container the supervisor started, reached
  `NeoSense supervisor ready`, and created `/run/neosense/neosensed.sock` as
  `srw-rw---- root:neosense`.
- A non-privileged container could not start the service because the systemd
  sandbox requires `CAP_SYS_ADMIN` for its mount namespace; this is a container
  capability limitation, not an installer failure.
- NBFC was absent, so verification correctly reported `BACKEND_UNAVAILABLE`
  and manual control unavailable. No fan targets were issued.
- A second install with `--software-only --no-restart` created a backup and left
  the running supervisor PID unchanged.
- An injected unit-install failure restored the previous binaries and unit and
  left the supervisor running.
- Uninstall removed binaries/unit/runtime and preserved custom profile and
  validation test files; the empty group was removed. NBFC was untouched.

This validates software installation, systemd integration, socket setup,
upgrade, rollback and uninstall without a real fan backend or physical fan
actions. It does **not** replace a VM or real hardware validation.

## Real Fedora 44 VM validation (2026-11-18)

A genuine disposable Fedora 44 x86_64 KVM VM was provisioned from the official
Fedora Cloud Base Generic 44-1.7 qcow2. The official `Fedora-Cloud-44-1.7-x86_64-CHECKSUM`
was downloaded and verified before boot.

- 2 vCPU, 4096 MiB RAM, 20 GiB qcow2 overlay, QEMU/KVM.
- QEMU user-mode networking with SSH on localhost port 2222.
- Separate root filesystem, package database and systemd PID 1
  (`Fedora Linux 44 (Cloud Edition)`, systemd 259).
- No PCI, USB, EC or fan-control device passthrough. No production files,
  profiles, validation records or services were mounted.
- Release archive: `neosense-2.4.0-x86_64-unknown-linux-gnu.tar.gz`. Its
  `BUILD-INFO` names the exact source commit, target, profile and toolchain; it
  was checked inside the VM before installation. The archive was built twice and
  matched byte for byte, and the adjacent `SHA256SUMS` records its digest.
- The same VM first validated the archive built from the earlier candidate
  `8e11a03`; the final archive was then installed and verified over it, so the
  evidence below covers the shipped binaries.

Results:

- `neosense.sh install --yes --software-only --no-start` installed binaries,
  unit, group and runtime/state directories. Exit 7 was the expected
  `BACKEND_UNAVAILABLE` status.
- `systemctl enable --now neosensed` reached active/running and
  `NeoSense supervisor ready`; the socket was `srw-rw---- root:neosense`.
- A non-root session in the `neosense` group could read status and use the CLI.
- The TUI was captured over a real PTY at 120×40 and 60×18: the masthead-only
  header rendered, and the fan panel reported
  `Fan state unavailable: NBFC status unavailable` instead of claiming the
  healthy supervisor was unreachable.
- `neosense.sh verify` reported `BACKEND_UNAVAILABLE`; no fan control was
  claimed.
- Upgrade with `--software-only --no-restart` preserved the supervisor PID,
  created a backup and retained test profiles/validation files.
- An injected unit-install failure restored the previous binaries/unit and left
  the running supervisor PID intact (exit 1).
- Uninstall removed binaries/unit/runtime, preserved profiles and validation,
  kept the group while it still had members, and did not touch NBFC.
- The official `nbfc-linux` 0.5.4 Fedora 44 RPM was checksum-verified and
  installed in the VM. The package metadata lists an unsigned RPM, and its
  dependencies were installed. `nbfc_service` could not start because the VM
  has no `/etc/nbfc/nbfc.json` and no compatible EC hardware; no socket was
  created and backend readiness remained unavailable.
- A default (non-software-only) install with NBFC installed but its service
  unavailable stopped before changing NeoSense software, as designed.

No fan, EC, ACPI, WMI, PWM or firmware operation was issued in the VM or on the
host.

## Supervised fresh-install trial (pending)

A **backend-ready** trial on a disposable Nobara 44/Fedora 44 VM remains
pending because the VM has no compatible fan/EC hardware. The software,
systemd, socket, upgrade, rollback and uninstall paths above are validated. The
following procedure remains the checklist for the hardware-backed trial.

**Prerequisites**

- Fresh VM or spare machine with systemd and network access to configured
  distribution repositories; take a snapshot first.
- No existing NeoSense installation.
- For a backend-path trial, make a verified NBFC 0.5.4+ package or artifact
  available in the VM. For a software-only trial, no backend is required.
- A checksum-verified release archive from the approved commit, or a clean
  source checkout.

**Commands**

```bash
sha256sum -c SHA256SUMS
tar -xzf neosense-2.4.0-x86_64-unknown-linux-gnu.tar.gz
cd neosense-2.4.0-x86_64-unknown-linux-gnu
sudo ./neosense.sh --dry-run
sudo ./neosense.sh --yes --no-start
./neosense.sh verify
```

Use `--software-only` for a VM without NBFC. Use `--install-backend` only when
a verified package is available in the VM's configured repositories.

**Expected evidence**

- Dependency report shows the source/prebuilt path and no unresolved mandatory
  package.
- Backend state is `ready`, `read_only`, or explicitly `missing`/`unavailable`;
  the result is never reported as full fan control unless the backend is ready.
- `systemctl show neosensed` shows intended unit state; the socket is
  `root:neosense` and restricted.
- `neosense diagnose --json` is captured read-only.
- `./neosense.sh verify` returns `READY_WITH_LIMITATIONS` or a specific
  backend/control blocker.
- Hash `/etc/nbfc` before and after to prove it was untouched unless an
  explicitly authorized backend package was installed.
- Journal contains no `fan.set`, `preset.apply` or `profile.apply` from the
  trial.

**Failure/rollback checks**

- Inject a failed binary/unit install in an isolated copy and confirm
  `install.sh` restores the previous files.
- Uninstall the VM installation and confirm profiles/validation are preserved.
- Restore the snapshot if the VM's backend condition becomes unclear.

Do not use this procedure on the production Acer installation.
