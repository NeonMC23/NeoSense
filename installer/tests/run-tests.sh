#!/usr/bin/env bash
# Isolated integration tests for the unified NeoSense installer.
# No package installation, service change, backend change or fan write is made.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
cleanup() { rm -rf "${TMP}"; }
trap cleanup EXIT

FAKEBIN="${TMP}/bin"
DMI="${ROOT}/installer/fixtures/dmi"
mkdir -p "${FAKEBIN}" "${TMP}/prefix/bin" "${TMP}/systemd" "${TMP}/run" "${TMP}/state"

cat > "${TMP}/prefix/bin/neosense" <<'FAKE'
#!/bin/sh
case "$*" in
    *--version*) echo "neosense 2.4.0" ;;
    *"service status"*) echo '{}' ;;
    *"diagnose --json"*) cat <<'JSON'
{
  "summary": {
    "fan_control_available": true,
    "physical_feedback_available": false,
    "physical_identity_verified": false,
    "physical_control_validated": false,
    "critical_issues": 0
  },
  "control": {"stage": "hardware_unvalidated"}
}
JSON
        ;;
    *) exit 0 ;;
esac
FAKE
chmod +x "${TMP}/prefix/bin/neosense"
cat > "${TMP}/prefix/bin/neosensed" <<'FAKE'
#!/bin/sh
echo "neosensed 2.4.0"
FAKE
chmod +x "${TMP}/prefix/bin/neosensed"
touch "${TMP}/systemd/neosensed.service"
python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.listen(1)' "${TMP}/run/neosensed.sock"

cat > "${FAKEBIN}/systemctl" <<'FAKE'
#!/bin/sh
command="$1"
shift || true
case "${command}" in
    --version) echo "systemd 259"; exit 0 ;;
    is-active)
        service=""
        for arg in "$@"; do
            case "${arg}" in --quiet|--no-pager|--value) ;; *) service="${arg}"; break ;; esac
        done
        case "${service}" in
            nbfc_service) [ "${FAKE_NBFC_ACTIVE:-1}" = "1" ] ;;
            neosensed) [ "${FAKE_SUPERVISOR_ACTIVE:-1}" = "1" ] ;;
            fancontrol|thinkfan|coolercontrol) [ "${FAKE_CONFLICT:-0}" = "1" ] ;;
            *) exit 1 ;;
        esac
        ;;
    is-enabled) exit 0 ;;
    daemon-reload|enable|start|restart|stop) exit 0 ;;
    *) exit 0 ;;
esac
FAKE
chmod +x "${FAKEBIN}/systemctl"

cat > "${FAKEBIN}/nbfc" <<'FAKE'
#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "nbfc ${FAKE_NBFC_VERSION:-0.5.4}"
    exit 0
fi
exit 1
FAKE
chmod +x "${FAKEBIN}/nbfc"

cat > "${FAKEBIN}/rustc" <<'FAKE'
#!/bin/sh
echo "rustc 1.99.0 (test toolchain)"
FAKE
chmod +x "${FAKEBIN}/rustc"
cat > "${FAKEBIN}/cargo" <<'FAKE'
#!/bin/sh
echo "cargo 1.99.0 (test toolchain)"
FAKE
chmod +x "${FAKEBIN}/cargo"
cat > "${FAKEBIN}/cc" <<'FAKE'
#!/bin/sh
echo "cc (GCC) 15.0.0"
FAKE
chmod +x "${FAKEBIN}/cc"
cat > "${FAKEBIN}/ld" <<'FAKE'
#!/bin/sh
echo "GNU ld version 2.43"
FAKE
chmod +x "${FAKEBIN}/ld"

cat > "${FAKEBIN}/dnf" <<'FAKE'
#!/bin/sh
case " $* " in
    *" info "*) [ "${FAKE_PKG_AVAILABLE:-0}" = "1" ] ;;
    *" install "*) [ "${FAKE_PKG_INSTALL:-0}" = "1" ] ;;
    *) exit 0 ;;
esac
FAKE
chmod +x "${FAKEBIN}/dnf"

PASS=0
FAIL=0
FAILED_NAMES=()

pass() { PASS=$((PASS + 1)); printf 'ok  %s\n' "$1"; }
fail() { FAIL=$((FAIL + 1)); FAILED_NAMES+=("$1"); printf 'not ok  %s\n' "$1"; shift || true; [ "$#" -gt 0 ] && printf '    %s\n' "$*"; }

assert_contains() {
    local name="$1" needle="$2" haystack="$3"
    if printf '%s' "${haystack}" | grep -Fq -- "${needle}"; then pass "${name}"; else fail "${name}" "missing: ${needle}"; fi
}

assert_code() {
    local name="$1" expected="$2" actual="$3"
    if [ "${actual}" = "${expected}" ]; then pass "${name}"; else fail "${name}" "expected exit ${expected}, got ${actual}"; fi
}

ns_env() {
    local os="$1"; shift
    env \
        PATH="${FAKEBIN}:${PATH}" \
        NEOSENSE_OS_RELEASE_FILE="${ROOT}/installer/fixtures/os-release-${os}" \
        NEOSENSE_DMI_DIR="${DMI}" \
        NEOSENSE_ARCH="${TEST_ARCH:-x86_64}" \
        NEOSENSE_SYSTEMD="${TEST_SYSTEMD:-1}" \
        NEOSENSE_SYSTEMD_VERSION=259 \
        NEOSENSE_PM="${TEST_PM:-dnf}" \
        NEOSENSE_NBFC_BIN="${TEST_NBFC_BIN:-${FAKEBIN}/nbfc}" \
        NEOSENSE_NBFC_VERSION="${TEST_NBFC_VERSION:-0.5.4}" \
        NEOSENSE_NBFC_SERVICE_ACTIVE="${FAKE_NBFC_ACTIVE:-1}" \
        NEOSENSE_NBFC_SOCKET_EXISTS=1 \
        NEOSENSE_NBFC_SOCKET_MODE="${TEST_SOCKET_MODE:-600}" \
        NEOSENSE_NBFC_CONFIG_EXISTS=1 \
        NEOSENSE_PREFIX="${TMP}/prefix" \
        NEOSENSE_SYSTEMD_DIR="${TMP}/systemd" \
        NEOSENSE_RUNTIME_DIR="${TMP}/run" \
        NEOSENSE_STATE_DIR="${TMP}/state" \
        NEOSENSE_GROUP="$(id -gn)" \
        NEOSENSE_TEST_MISSING_DEP="${TEST_MISSING:-}" \
        NEOSENSE_TEST_OLD_DEP="${TEST_OLD:-}" \
        "${ROOT}/neosense.sh" "$@"
}

# Help and argument handling
set +e
out="$(ns_env nobara --help 2>&1)"; code=$?; set -e
assert_code "help exits 0" 0 "${code}"
assert_contains "help lists install" "install" "${out}"
assert_contains "help lists diagnostics" "diagnose" "${out}"

set +e
out="$(ns_env nobara nonsense 2>&1)"; code=$?; set -e
assert_code "unknown command rejected" 2 "${code}"

# Self-location from an arbitrary working directory
out="$(cd "${TMP}" && ns_env nobara detect)"
assert_contains "self-locates outside the repository" "family               fedora" "${out}"

# Distribution detection
out="$(ns_env nobara detect)"; assert_contains "Nobara is Fedora family" "family               fedora" "${out}"
assert_contains "Nobara package manager" "package manager      dnf" "${out}"
assert_contains "Nobara host model" "Nitro AN515-57" "${out}"
assert_contains "Nobara board" "Scala_TLS" "${out}"

out="$(TEST_PM=apt ns_env debian detect)"; assert_contains "Debian family" "family               debian" "${out}"
out="$(TEST_PM=pacman ns_env arch detect)"; assert_contains "Arch family" "family               arch" "${out}"
out="$(TEST_PM=zypper ns_env opensuse detect)"; assert_contains "openSUSE family" "family               suse" "${out}"
out="$(TEST_PM=dnf ns_env unknown detect)"; assert_contains "Unknown family" "family               unknown" "${out}"

out="$(TEST_ARCH=riscv64 ns_env nobara detect)"; assert_contains "Unsupported architecture" "installation         not supported" "${out}"
out="$(TEST_SYSTEMD=0 ns_env nobara detect)"; assert_contains "Missing systemd detected" "systemd              0" "${out}"
out="$(TEST_PM=none ns_env nobara detect)"; assert_contains "Missing package manager" "package manager      none" "${out}"

set +e
out="$(TEST_ARCH=riscv64 ns_env nobara install --dry-run 2>&1)"; code=$?; set -e
assert_code "unsupported architecture blocks install" 1 "${code}"
assert_contains "unsupported architecture reason" "not supported" "${out}"

set +e
out="$(TEST_SYSTEMD=0 ns_env nobara install --dry-run 2>&1)"; code=$?; set -e
assert_code "missing systemd blocks install" 1 "${code}"

# Unprivileged install must be refused before changes.
if [ "$(id -u)" -ne 0 ]; then
    set +e
    out="$(ns_env nobara install 2>&1)"; code=$?; set -e
    assert_code "unprivileged install refused" 1 "${code}"
    assert_contains "unprivileged install hint" "sudo" "${out}"
fi

# Package-manager adapter dry-runs
for adapter in apt pacman zypper dnf; do
    out="$(NEOSENSE_DRY_RUN=1 bash -c '. "$1/installer/lib/common.sh"; . "$1/installer/lib/pkg.sh"; PM="$2"; PM_FAMILY="$2"; pkg_install demo' _ "${ROOT}" "${adapter}")"
    assert_contains "${adapter} adapter dry-run" "+ ${adapter} demo" "${out}"
done

# Dependency reports and seams
out="$(ns_env nobara deps)"; assert_contains "source dependency mode" "source installation" "${out}"
assert_contains "Rust dependency present" "rustc" "${out}"

set +e
out="$(TEST_MISSING=tar ns_env nobara deps 2>&1)"; code=$?; set -e
assert_code "missing mandatory dependency fails" 1 "${code}"
assert_contains "missing dependency listed" "MISSING" "${out}"

set +e
out="$(TEST_MISSING=nvidia-smi ns_env nobara deps 2>&1)"; code=$?; set -e
assert_code "missing optional dependency is not fatal" 0 "${code}"
assert_contains "optional dependency status" "OPTIONAL_MISSING" "${out}"

set +e
out="$(TEST_OLD=rustc ns_env nobara deps 2>&1)"; code=$?; set -e
assert_code "old dependency fails" 1 "${code}"
assert_contains "old dependency status" "OLD" "${out}"

# Backend states (read-only verify against the installed binaries; fake backend).
set +e
out="$(ns_env nobara verify 2>&1)"; code=$?; set -e
assert_contains "ready backend detected" "nbfc 0.5.4" "${out}"
assert_contains "verify reports hardware limitation" "HARDWARE_NOT_VALIDATED" "${out}"

set +e
out="$(TEST_NBFC_VERSION=0.5.3 ns_env nobara verify 2>&1)"; code=$?; set -e
assert_contains "NBFC 0.5.3 is read-only" "read_only" "${out}"

set +e
out="$(FAKE_CONFLICT=1 ns_env nobara verify 2>&1)"; code=$?; set -e
assert_contains "competing controller detected" "conflict" "${out}"
assert_contains "conflicting controller result" "CONTROL_NOT_ELIGIBLE" "${out}"

set +e
out="$(FAKE_NBFC_ACTIVE=0 ns_env nobara verify 2>&1)"; code=$?; set -e
assert_contains "inactive backend detected" "unavailable" "${out}"

# Missing backend blocks install before software changes.
set +e
out="$(TEST_NBFC_BIN=/nonexistent ns_env nobara install --dry-run 2>&1)"; code=$?; set -e
assert_code "missing backend blocks install" 7 "${code}"
assert_contains "missing backend guidance" "NBFC is required" "${out}"

set +e
out="$(TEST_NBFC_BIN=/nonexistent ns_env nobara install --dry-run --software-only 2>&1)"; code=$?; set -e
assert_contains "software-only install proceeds" "software only" "${out}"
assert_contains "software-only dry-run plan" "install -Dm755" "${out}"

# Backend package path is explicit and dry-run safe.
set +e
out="$(TEST_NBFC_BIN=/nonexistent FAKE_PKG_AVAILABLE=0 ns_env nobara install --dry-run --install-backend --yes 2>&1)"; code=$?; set -e
assert_code "unavailable backend package blocks install" 7 "${code}"
assert_contains "backend package guidance" "NBFC is required" "${out}"

set +e
out="$(TEST_NBFC_BIN=/nonexistent FAKE_PKG_AVAILABLE=1 ns_env nobara install --dry-run --install-backend --yes --software-only 2>&1)"; code=$?; set -e
assert_contains "backend package dry-run command" "+ dnf nbfc" "${out}"

# Dry-run software install and uninstall are non-destructive.
set +e
out="$(ns_env nobara install --dry-run --no-start 2>&1)"; code=$?; set -e
assert_contains "dry-run software plan" "install -Dm755" "${out}"
assert_contains "dry-run reports no changes" "Dry-run" "${out}"

set +e
out="$(ns_env nobara uninstall --dry-run --yes 2>&1)"; code=$?; set -e
assert_contains "uninstall dry-run" "NeoSense has been uninstalled" "${out}"
assert_contains "uninstall preserves NBFC" "nbfc_service" "${out}"

printf '\n%d passed, %d failed\n' "${PASS}" "${FAIL}"
if [ "${FAIL}" -gt 0 ]; then
    printf 'failed: %s\n' "${FAILED_NAMES[*]}" >&2
    exit 1
fi
