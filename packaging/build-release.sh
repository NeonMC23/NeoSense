#!/usr/bin/env bash
# Build a reproducible, self-contained NeoSense release archive.
set -euo pipefail
umask 022

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
if git rev-parse --git-dir >/dev/null 2>&1; then
    if ! git diff --quiet || ! git diff --cached --quiet; then
        printf 'Refusing to build from a dirty worktree; commit or discard changes first.\n' >&2
        exit 1
    fi
fi
cargo build --release --locked

VERSION="$(target/release/neosense --version | awk '{print $2}')"
case "${VERSION}" in
    ""|*/*|*..*)
        printf 'Refusing to build with an unsafe version string: %s\n' "${VERSION}" >&2
        exit 1
        ;;
esac
COMMIT="$(git rev-parse HEAD 2>/dev/null || printf 'unknown')"
COMMIT_SHORT="$(git rev-parse --short=12 HEAD 2>/dev/null || printf 'unknown')"
TARGET="$(rustc -vV | sed -n 's/^host: //p')"
SOURCE_TIME="$(git show -s --format=%cI HEAD 2>/dev/null || date -u +%Y-%m-%dT%H:%M:%SZ)"
SOURCE_EPOCH="$(git show -s --format=%ct HEAD 2>/dev/null || date +%s)"
NAME="neosense-${VERSION}-${TARGET}"
STAGE="${OUTPUT}/${NAME}"
rm -rf "${STAGE}"
mkdir -p "${STAGE}/bin" "${STAGE}/packaging" "${STAGE}/docs" "${STAGE}/completions" "${STAGE}/installer/lib"

install -m755 target/release/neosense "${STAGE}/bin/neosense"
install -m755 target/release/neosensed "${STAGE}/bin/neosensed"
install -m755 install.sh uninstall.sh neosense.sh "${STAGE}/"
install -m644 installer/lib/*.sh "${STAGE}/installer/lib/"
install -m644 README.md LICENSE CHANGELOG.md "${STAGE}/"
for extra in SECURITY.md CONTRIBUTING.md; do
    if [ -f "${extra}" ]; then
        install -m644 "${extra}" "${STAGE}/"
    fi
done
install -m644 packaging/neosensed.service "${STAGE}/packaging/"
install -m644 docs/*.md "${STAGE}/docs/"

for shell in bash zsh fish elvish powershell; do
    target/release/neosense completions "${shell}" > "${STAGE}/completions/neosense.${shell}"
    chmod 0644 "${STAGE}/completions/neosense.${shell}"
done

printf 'package=neosense\nversion=%s\ncommit=%s\ncommit_short=%s\ntarget=%s\nprofile=release\nsource_time=%s\nrustc=%s\ncargo=%s\n' \
    "${VERSION}" "${COMMIT}" "${COMMIT_SHORT}" "${TARGET}" "${SOURCE_TIME}" \
    "$(rustc --version)" "$(cargo --version)" \
    > "${STAGE}/BUILD-INFO"
chmod 0644 "${STAGE}/BUILD-INFO"

ARCHIVE="${OUTPUT}/${NAME}.tar.gz"
tar --sort=name --mtime="@${SOURCE_EPOCH}" --owner=0 --group=0 --numeric-owner \
    -czf "${ARCHIVE}" -C "${OUTPUT}" "${NAME}"
(cd "${OUTPUT}" && sha256sum "$(basename "${ARCHIVE}")" > SHA256SUMS)
printf '%s\n' "${ARCHIVE}"
