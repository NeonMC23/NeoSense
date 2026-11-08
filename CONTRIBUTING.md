# Contributing to NeoSense

Thank you for helping improve NeoSense. The project prioritises thermal safety,
truthful telemetry and predictable state management over feature count.

## Before contributing

- Read `README.md`, `docs/IMPLEMENTATION.md`, `docs/SYSTEMD.md` and `docs/TUI.md`.
- Hardware-sensitive changes require a clear safety analysis.
- Never add a manual fan speed or profile that bypasses `neosensed`.
- Never claim RPM, GPU control or compatibility without evidence.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release --locked
shellcheck install.sh uninstall.sh packaging/build-release.sh
```

Use fixtures and fake backends for failure scenarios. Do not run fan-speed,
EC-reset or firmware tests as part of ordinary development.

## Pull requests

- Keep changes focused and explain the safety impact.
- Add regression tests for fixed bugs.
- Update documentation and `docs/ROADMAP.md` when status changes.
- Do not commit `target/`, release archives, logs or machine-specific data.

## Reporting issues

Include `neosense diagnose --json` output after reviewing it for serial numbers,
hostnames or other private data. State whether the result came from real hardware
or a fake backend.
