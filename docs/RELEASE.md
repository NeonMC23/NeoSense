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

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-targets --all-features`
- [ ] `cargo build --release --locked`
- [ ] ShellCheck and Bash syntax
- [ ] `systemd-analyze verify` with neutralized paths/group
- [ ] Real read-only diagnostics on target hardware
- [ ] Root installation/upgrade/uninstall on a disposable VM
- [ ] `packaging/test-installer-isolated.sh` on a namespace-capable host
- [ ] `packaging/test-release-archive.sh` on the generated archive
- [ ] TUI screenshot/session review
- [ ] Release notes state limitations accurately
- [ ] Checksums attached
- [ ] Git tag and remote verified

## Known release blockers

- Root host installation has not been validated because sudo credentials are unavailable in this environment.
- Manual fan control is intentionally disabled on the current NBFC 0.5.3 hardware.
- Closed-loop profiles remain preview-only.
- NBFC socket 0666 remains an upstream security limitation.
