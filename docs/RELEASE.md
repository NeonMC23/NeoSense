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
- [x] TUI rendered review at 40×20, 80×24, 100×32 and 160×48 (no image screenshot committed)
- [x] Release notes state limitations accurately
- [x] Disposable root container: install, READY, socket, group, watchdog, stop and SIGKILL recovery
- [ ] Checksums attached
- [ ] Git tag and remote verified

## Known release blockers

- Root host installation has not been validated because sudo credentials are unavailable in this environment.
- Manual fan control is intentionally disabled on the current NBFC 0.5.3 hardware.
- Closed-loop profiles remain preview-only.
- NBFC socket 0666 remains an upstream security limitation.
