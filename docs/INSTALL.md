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
| Nobara 44 / Fedora | Implemented (DNF) | Mock-tested; host installation already present, not re-deployed | Acer Nitro AN515-57 is the primary tested machine; manual control remains policy-gated |
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
| `READY_WITH_LIMITATIONS` | Software, backend and hardware validated, but independent physical feedback is limited |
| `HARDWARE_NOT_VALIDATED` | Hardware evidence is insufficient for the full claim |
| `BACKEND_UNAVAILABLE` | NBFC is missing, inactive or unusable |
| `CONTROL_NOT_ELIGIBLE` | Policy or a competing controller blocks manual control |
| `INSTALLATION_INCOMPLETE` | Binaries, unit, version or IPC checks failed |

The current project cannot honestly report `READY` on the Acer laptop because
independent RPM/airflow and physical fan identity remain unvalidated. Expect
`HARDWARE_NOT_VALIDATED` when everything software-side is correct.

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
- Manual control: policy-eligible on this installation, but physical control
  remains operator-attested only.
- Independent RPM/airflow: unavailable. EC tach words are provisional and never
  treated as RPM.
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
