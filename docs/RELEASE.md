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
- [x] TUI rendered review at 40×20, 80×24, 100×32 and 160×48; faithful PNG renders inspected (files not committed)
- [x] Release notes state limitations accurately
- [x] Disposable root container: install, READY, socket, group, watchdog, stop and SIGKILL recovery
- [x] Historical 2.2.0 -> 2.3.0 upgrade and injected rollback
- [x] Active-supervisor restart verification and failure recovery
- [ ] Checksums attached
- [ ] Git tag and remote verified

## Known release blockers

- Host installation is validated on the development machine (repaired 2.4.0, 2026-11-11); other machines still need their own validation.
- Manual fan control remains disabled: NBFC 0.5.4 is installed, but the AN515-57 read scale, physical identity and RPM feedback are not validated.
- Closed-loop profiles remain preview-only.
- Unpatched upstream NBFC still creates its socket as 0666; the host runs the reviewed 0600 patch.
