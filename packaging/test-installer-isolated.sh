#!/usr/bin/env bash
# Isolated installer/rollback test. Uses a user namespace, temporary prefixes
# and a deliberately failing `install` wrapper. It never touches host files or
# hardware. Requires unshare -r and must not be run as a normal CI test on
# hosts that disallow user namespaces.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
cleanup() { rm -rf "${TMP}"; }
trap cleanup EXIT

if ! unshare -r true >/dev/null 2>&1; then
    printf 'SKIP: unshare -r is unavailable\n'
    exit 0
fi

mkdir -p "${TMP}/fakebin" "${TMP}/fake-systemctl"
cat > "${TMP}/fakebin/install" <<'WRAPPER'
#!/bin/sh
case "$*" in
  *neosensed.service*) echo "injected unit failure" >&2; exit 1 ;;
esac
exec /usr/bin/install "$@"
WRAPPER
chmod +x "${TMP}/fakebin/install"
cat > "${TMP}/fake-systemctl/systemctl" <<'WRAPPER'
#!/bin/sh
if [ "${1:-}" = "daemon-reload" ]; then
    echo "injected daemon-reload failure" >&2
    exit 1
fi
exit 0
WRAPPER
chmod +x "${TMP}/fake-systemctl/systemctl"

unshare -r --fork bash -s <<INNER
set -euo pipefail
cd "${ROOT}"
export NEOSENSE_PREFIX="${TMP}/prefix"
export NEOSENSE_SYSTEMD_DIR="${TMP}/systemd"
export NEOSENSE_RUNTIME_DIR="${TMP}/run"
export NEOSENSE_STATE_DIR="${TMP}/state"
export NEOSENSE_GROUP=root
export NEOSENSE_SKIP_SYSTEMCTL=1

./install.sh >"${TMP}/install.log"
test -x "\${NEOSENSE_PREFIX}/bin/neosense"
test -f "\${NEOSENSE_SYSTEMD_DIR}/neosensed.service"
printf 'old-binary\n' > "\${NEOSENSE_PREFIX}/bin/neosense"
printf 'old-unit\n' > "\${NEOSENSE_SYSTEMD_DIR}/neosensed.service"

set +e
PATH="${TMP}/fakebin:\$PATH" ./install.sh >"${TMP}/failed.out" 2>"${TMP}/failed.err"
code=\$?
set -e
test "\$code" -ne 0
test "\$(cat "\${NEOSENSE_PREFIX}/bin/neosense")" = "old-binary"
test "\$(cat "\${NEOSENSE_SYSTEMD_DIR}/neosensed.service")" = "old-unit"

printf 'old-binary-2\n' > "\${NEOSENSE_PREFIX}/bin/neosense"
printf 'old-unit-2\n' > "\${NEOSENSE_SYSTEMD_DIR}/neosensed.service"
rm -rf "\${NEOSENSE_RUNTIME_DIR}"
set +e
PATH="${TMP}/fake-systemctl:\$PATH" NEOSENSE_SKIP_SYSTEMCTL=0 ./install.sh >"${TMP}/reload-failed.out" 2>"${TMP}/reload-failed.err"
code=\$?
set -e
test "\$code" -ne 0
grep -q "systemctl daemon-reload failed" "${TMP}/reload-failed.err"
test "\$(cat "\${NEOSENSE_PREFIX}/bin/neosense")" = "old-binary-2"
test "\$(cat "\${NEOSENSE_SYSTEMD_DIR}/neosensed.service")" = "old-unit-2"
test ! -e "\${NEOSENSE_RUNTIME_DIR}"

./install.sh >"${TMP}/reinstall.log"
./uninstall.sh >"${TMP}/uninstall.log"
test ! -e "\${NEOSENSE_PREFIX}/bin/neosense"
test ! -e "\${NEOSENSE_SYSTEMD_DIR}/neosensed.service"
echo "isolated installer/rollback/uninstall: OK"
INNER
