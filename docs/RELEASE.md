# NeoSense release process

## Build

```bash
./packaging/build-release.sh
```

The script runs a locked release build and creates:

- `dist/neosense-<version>-<target>.tar.gz`
- extracted binaries, installer, unit, docs and completions
- `BUILD-INFO`
- `SHA256SUMS`

It does not commit or publish artifacts.

## Release checklist

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --all-targets --all-features -- -D warnings`
- [x] `cargo test --all-targets --all-features`
- [x] `cargo build --release --locked`
- [x] ShellCheck and Bash syntax
- [x] `systemd-analyze verify` with neutralized paths/group
- [x] Real read-only diagnostics on target hardware
- [ ] Root installation/upgrade/uninstall on a disposable VM
- [x] `packaging/test-installer-isolated.sh` on a namespace-capable host
- [x] `packaging/test-release-archive.sh` on the generated archive
- [x] TUI rendered review at 60×18, 80×24 and 120×40 in a real pty and with `TestBackend` at 20–160 columns
- [x] Release notes state limitations accurately
- [x] Disposable root container: install, READY, socket, group, watchdog, stop and SIGKILL recovery
- [x] Historical 2.2.0 -> 2.3.0 upgrade and injected rollback
- [x] Active-supervisor restart verification and failure recovery
- [ ] Checksums attached
- [ ] Git tag and remote verified

## Known release blockers

- Host installation is validated on the development machine (`897e757`, deployed 2026-11-17); other machines still need their own validation.
- Manual and closed-loop profile writes are implemented but per-fan policy-gated. The GPU logical channel can use a trusted independent NVIDIA GPU temperature when NBFC declares `@CPU`. CPU and GPU actuator response are operator-attested; independent RPM/airflow calibration remains outstanding.
- Unpatched upstream NBFC still creates its socket as 0666; the host runs the reviewed 0600 patch.
