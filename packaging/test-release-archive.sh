#!/usr/bin/env bash
# Validate a release archive without using the source checkout as the install
# source. Extraction and prebuilt installation run in a temporary directory;
# the install/uninstall path additionally uses a user namespace when available.
set -euo pipefail

ARCHIVE="${1:-}"
if [ -z "${ARCHIVE}" ]; then
    printf 'Usage: %s ARCHIVE.tar.gz\n' "$0" >&2
    exit 2
fi
if [ ! -f "${ARCHIVE}" ]; then
    printf 'Archive not found: %s\n' "${ARCHIVE}" >&2
    exit 2
fi

TMP="$(mktemp -d)"
cleanup() { rm -rf "${TMP}"; }
trap cleanup EXIT

tar -tzf "${ARCHIVE}" > "${TMP}/contents"
TOP="$(sed -n '1s#/.*##p' "${TMP}/contents")"
test -n "${TOP}"
for required in \
    "${TOP}/install.sh" \
    "${TOP}/uninstall.sh" \
    "${TOP}/bin/neosense" \
    "${TOP}/bin/neosensed" \
    "${TOP}/packaging/neosensed.service" \
    "${TOP}/docs/CLI.md" \
    "${TOP}/completions/neosense.bash" \
    "${TOP}/completions/neosense.zsh" \
    "${TOP}/completions/neosense.fish" \
    "${TOP}/completions/neosense.elvish" \
    "${TOP}/completions/neosense.powershell" \
    "${TOP}/BUILD-INFO"; do
    grep -qx "${required}" "${TMP}/contents" || {
        printf 'Missing archive member: %s\n' "${required}" >&2
        exit 1
    }
done
if grep -Eq '(^|/)(target|\.git|src|tests)(/|$)' "${TMP}/contents"; then
    printf 'Development files leaked into the archive.\n' >&2
    exit 1
fi

tar -xzf "${ARCHIVE}" -C "${TMP}"
VERSION="$("${TMP}/${TOP}/bin/neosense" --version | awk '{print $2}')"
test -n "${VERSION}"
grep -qx "version=${VERSION}" "${TMP}/${TOP}/BUILD-INFO"
grep -Eq '^commit=[0-9a-f]{40}$' "${TMP}/${TOP}/BUILD-INFO" || {
    printf 'BUILD-INFO does not contain a full source commit.\n' >&2
    exit 1
}

if unshare -r true >/dev/null 2>&1; then
    unshare -r --fork bash -s <<INNER
set -euo pipefail
cd "${TMP}/${TOP}"
export NEOSENSE_PREFIX="${TMP}/installed/prefix"
export NEOSENSE_SYSTEMD_DIR="${TMP}/installed/systemd"
export NEOSENSE_RUNTIME_DIR="${TMP}/installed/run"
export NEOSENSE_STATE_DIR="${TMP}/installed/state"
export NEOSENSE_GROUP=root
export NEOSENSE_SKIP_SYSTEMCTL=1
./install.sh >"${TMP}/archive-install.log"
test -x "\${NEOSENSE_PREFIX}/bin/neosense"
test -x "\${NEOSENSE_PREFIX}/bin/neosensed"
grep -q 'Group=root' "\${NEOSENSE_SYSTEMD_DIR}/neosensed.service"
test "\$("\${NEOSENSE_PREFIX}/bin/neosense" --version | awk '{print \$2}')" = "${VERSION}"
./uninstall.sh >"${TMP}/archive-uninstall.log"
test ! -e "\${NEOSENSE_PREFIX}/bin/neosense"
test ! -e "\${NEOSENSE_SYSTEMD_DIR}/neosensed.service"
INNER
    printf 'archive inspect + prebuilt install/uninstall: OK\n'
else
    printf 'archive inspect: OK (user namespaces unavailable; install path skipped)\n'
fi
