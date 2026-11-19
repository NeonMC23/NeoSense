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
- [x] Root software-only installation, upgrade, rollback and uninstall on a disposable Fedora 44 KVM VM
- [x] `packaging/test-installer-isolated.sh` on a namespace-capable host
- [x] `installer/tests/run-tests.sh` distribution, dependency, backend and dry-run flow tests
- [x] `packaging/test-release-archive.sh` on the generated archive
- [x] TUI rendered review at 60×18, 80×24 and 120×40 in a real pty and with `TestBackend` at 20–160 columns
- [x] Branded ASCII header reviewed at 120×40; compact fallback verified at 80×24 and 60×18
- [x] Release archive reproduced twice with identical SHA-256
- [x] Release notes state limitations accurately
- [x] Disposable root container: install, READY, socket, group, watchdog, stop and SIGKILL recovery
- [x] Historical 2.2.0 -> 2.3.0 upgrade and injected rollback
- [x] Active-supervisor restart verification and failure recovery
- [x] Checksums attached
- [x] Git tag and remote verified

## Publishing a release

After explicit approval, tag the approved commit and upload the verified bundle.
This is the workflow used for `v2.4.0`; adjust the version for future releases:

```bash
./packaging/build-release.sh
(cd dist && sha256sum -c SHA256SUMS)
./packaging/test-release-archive.sh dist/neosense-2.4.0-x86_64-unknown-linux-gnu.tar.gz
git tag -a v2.4.0 -m "NeoSense 2.4.0"
git push origin v2.4.0
gh release create v2.4.0 \
  dist/neosense-2.4.0-x86_64-unknown-linux-gnu.tar.gz \
  dist/SHA256SUMS \
  --title "NeoSense 2.4.0" \
  --notes-file docs/RELEASE_NOTES_2.4.0.md
```

`v2.4.0` was published on 2026-11-19 from commit `669ff4c` with the archive and
`SHA256SUMS` attached. The tag and published assets are immutable; future work
uses a new version.

## Known release blockers

- Host installation and the systemd lifecycle are validated on the development
  machine, and the published 2.4.0 release is installed there. Other machines
  still need their own validation, and physical calibration remains separate.
- Automatic backend preparation is limited to a distro NBFC package with explicit authorization; no unverified upstream installer is downloaded.
- Manual and closed-loop profile writes are implemented but per-fan policy-gated. The GPU logical channel can use a trusted independent NVIDIA GPU temperature when NBFC declares `@CPU`. CPU and GPU actuator response are operator-attested; independent RPM/airflow calibration remains outstanding.
- Unpatched upstream NBFC still creates its socket as 0666; the host runs the reviewed 0600 patch.
