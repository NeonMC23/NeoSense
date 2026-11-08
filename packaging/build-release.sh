#!/usr/bin/env bash
# Build a reproducible, self-contained NeoSense release archive.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT="${1:-${ROOT}/dist}"
if [ "${OUTPUT}" = "--help" ] || [ "${OUTPUT}" = "-h" ]; then
    printf 'Usage: %s [OUTPUT_DIR]\n' "$0"
    exit 0
fi
case "${OUTPUT}" in
    /*) : ;;
    *) OUTPUT="${ROOT}/${OUTPUT}" ;;
esac
mkdir -p "${OUTPUT}"

cd "${ROOT}"
cargo build --release --locked

VERSION="$(target/release/neosense --version | awk '{print $2}')"
COMMIT="$(git rev-parse --short HEAD 2>/dev/null || printf 'unknown')"
TARGET="$(rustc -vV | sed -n 's/^host: //p')"
NAME="neosense-${VERSION}-${TARGET}"
STAGE="${OUTPUT}/${NAME}"
rm -rf "${STAGE}"
mkdir -p "${STAGE}/bin" "${STAGE}/packaging" "${STAGE}/docs" "${STAGE}/completions"

install -m755 target/release/neosense "${STAGE}/bin/neosense"
install -m755 target/release/neosensed "${STAGE}/bin/neosensed"
install -m755 install.sh uninstall.sh "${STAGE}/"
install -m644 README.md LICENSE CHANGELOG.md "${STAGE}/"
install -m644 packaging/neosensed.service "${STAGE}/packaging/"
install -m644 docs/*.md "${STAGE}/docs/"

for shell in bash zsh fish; do
    target/release/neosense completions "${shell}" > "${STAGE}/completions/neosense.${shell}" 2>/dev/null \
        || rm -f "${STAGE}/completions/neosense.${shell}"
done

printf 'version=%s\ncommit=%s\ntarget=%s\nbuilt=%s\n' \
    "${VERSION}" "${COMMIT}" "${TARGET}" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    > "${STAGE}/BUILD-INFO"

ARCHIVE="${OUTPUT}/${NAME}.tar.gz"
tar -czf "${ARCHIVE}" -C "${OUTPUT}" "${NAME}"
sha256sum "${ARCHIVE}" > "${OUTPUT}/SHA256SUMS"
printf '%s\n' "${ARCHIVE}"
