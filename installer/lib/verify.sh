#!/usr/bin/env bash
# Read-only post-install verification and diagnostic report.

# Resolved at verification time: the unified CLI parses --prefix/--systemd-dir
# after this library is sourced, so snapshots taken here would be stale.
V_PREFIX=""
V_SYSTEMD_DIR=""
V_GROUP=""
V_RUNTIME_DIR=""
V_SOCKET=""
V_BIN=""
V_DAEMON=""
V_UNIT=""
V_CHECK_FAIL=0
V_CHECK_WARN=0
V_HARDWARE_VALIDATED=no
V_MANUAL_ELIGIBLE=no
V_FEEDBACK_AVAILABLE=no
V_FEEDBACK_TRUSTED=no
V_IDENTITY_VERIFIED=no
V_SERVICE_ACTIVE=no
V_UNIT_PRESENT=no
V_BIN_VERSION=""
V_DAEMON_VERSION=""

verify_info() { printf '%-20s %s\n' "$1" "$2"; }
verify_ok() { log_ok "$*"; }
verify_warn() { V_CHECK_WARN=$((V_CHECK_WARN + 1)); log_warn "$*"; }
verify_fail() { V_CHECK_FAIL=$((V_CHECK_FAIL + 1)); log_error "$*"; }

verify_binaries() {
    if [ -x "${V_BIN}" ]; then
        V_BIN_VERSION="$("${V_BIN}" --version 2>/dev/null | awk '{print $2}' | head -n1)"
        verify_ok "CLI binary: ${V_BIN} (${V_BIN_VERSION:-unknown})"
    else
        verify_fail "CLI binary missing: ${V_BIN}"
    fi
    if [ -x "${V_DAEMON}" ]; then
        V_DAEMON_VERSION="$("${V_DAEMON}" --version 2>/dev/null | awk '{print $2}' | head -n1)"
        verify_ok "Supervisor binary: ${V_DAEMON} (${V_DAEMON_VERSION:-unknown})"
    else
        verify_fail "Supervisor binary missing: ${V_DAEMON}"
    fi
    if [ -n "${V_BIN_VERSION}" ] && [ "${V_BIN_VERSION}" != "${V_DAEMON_VERSION}" ]; then
        verify_fail "CLI and supervisor versions differ (${V_BIN_VERSION} / ${V_DAEMON_VERSION})"
    fi
}

verify_unit() {
    if [ -f "${V_UNIT}" ]; then
        V_UNIT_PRESENT=yes
        verify_ok "systemd unit: ${V_UNIT}"
    else
        verify_fail "systemd unit missing: ${V_UNIT}"
    fi
    if [ "${NEOSENSE_SKIP_SYSTEMCTL:-0}" -eq 0 ] && have systemctl; then
        if systemctl is-active --quiet neosensed 2>/dev/null; then
            V_SERVICE_ACTIVE=yes
            verify_ok "neosensed is active"
        else
            verify_warn "neosensed is not active"
        fi
        if [ "${V_UNIT_PRESENT}" = yes ] && ! systemctl is-enabled --quiet neosensed 2>/dev/null; then
            verify_warn "neosensed is not enabled at boot"
        fi
    else
        verify_warn "systemctl checks skipped (isolated test mode)"
    fi
}

verify_permissions() {
    if getent group "${V_GROUP}" >/dev/null 2>&1; then
        verify_ok "Service group exists: ${V_GROUP}"
    else
        verify_fail "Service group missing: ${V_GROUP}"
    fi
    if [ -S "${V_SOCKET}" ]; then
        local mode owner
        mode="$(path_mode "${V_SOCKET}")"
        owner="$(path_owner_group "${V_SOCKET}")"
        verify_ok "Supervisor socket: ${V_SOCKET} (${mode}, ${owner})"
        if [ "${mode}" = "666" ] || [ "${mode}" = "777" ]; then
            verify_warn "Supervisor socket is writable by every local user"
        fi
    else
        verify_warn "Supervisor socket missing: ${V_SOCKET}"
    fi
}

verify_runtime() {
    if [ ! -x "${V_BIN}" ]; then
        return 0
    fi
    local out status
    out="$(mktemp)"
    set +e
    "${V_BIN}" service status --json --socket "${V_SOCKET}" >"${out}" 2>/dev/null
    status=$?
    set -e
    if [ "${V_SERVICE_ACTIVE}" != "yes" ]; then
        verify_warn "Supervisor IPC check skipped because neosensed is not active"
    else
        case "${status}" in
            0|6)
                verify_ok "Supervisor IPC reachable (service status exit ${status})"
                ;;
            4)
                verify_warn "Supervisor IPC permission denied (run as a member of ${V_GROUP}, or use sudo for diagnostics)"
                ;;
            *)
                verify_fail "Supervisor IPC unavailable (service status exit ${status})"
                ;;
        esac
    fi
    rm -f "${out}"
}

verify_diagnostics() {
    [ -x "${V_BIN}" ] || return 0
    local out status
    out="$(mktemp)"
    set +e
    "${V_BIN}" diagnose --json --socket "${V_SOCKET}" >"${out}" 2>/dev/null
    status=$?
    set -e
    if [ "${status}" -eq 0 ] || [ "${status}" -eq 5 ] || [ "${status}" -eq 6 ]; then
        if grep -q '"fan_control_available": true' "${out}"; then
            V_MANUAL_ELIGIBLE=yes
        fi
        if grep -q '"physical_feedback_available": true' "${out}"; then
            V_FEEDBACK_AVAILABLE=yes
        fi
        if grep -q '"physical_feedback_trusted": true' "${out}"; then
            V_FEEDBACK_TRUSTED=yes
        fi
        if grep -q '"physical_identity_verified": true' "${out}"; then
            V_IDENTITY_VERIFIED=yes
        fi
        if grep -q '"physical_control_validated": true' "${out}"; then
            V_HARDWARE_VALIDATED=yes
        fi
        if grep -q '"physical_control_validated": false' "${out}"; then
            verify_warn "Independent RPM/airflow calibration and physical mapping remain unverified; operator-attested actuator response is documented separately"
        fi
        if grep -q '"critical_issues": 0' "${out}"; then
            verify_ok "Diagnostics report no critical issues"
        else
            verify_warn "Diagnostics report critical issues (see neosense diagnose)"
        fi
    elif [ "${status}" -eq 7 ] && [ "${V_SERVICE_ACTIVE}" != "yes" ]; then
        verify_warn "Diagnostics unavailable because the supervisor is not active"
    else
        verify_fail "Diagnostics unavailable (exit ${status})"
    fi
    rm -f "${out}"
}

verify_final_status() {
    if [ "${V_CHECK_FAIL}" -gt 0 ]; then
        printf 'INSTALLATION_INCOMPLETE\n'
        return 1
    fi
    if [ "${BACKEND_STATE}" = "missing" ] || [ "${BACKEND_STATE}" = "unavailable" ] || [ "${BACKEND_STATE}" = "incompatible" ]; then
        printf 'BACKEND_UNAVAILABLE\n'
        return 7
    fi
    if [ "${BACKEND_STATE}" = "conflict" ] || [ "${V_MANUAL_ELIGIBLE}" != "yes" ]; then
        printf 'CONTROL_NOT_ELIGIBLE\n'
        return 8
    fi
    if [ "${V_HARDWARE_VALIDATED}" != "yes" ]; then
        printf 'READY_WITH_LIMITATIONS\n'
        return 6
    fi
    if [ "${V_FEEDBACK_AVAILABLE}" != "yes" ]; then
        printf 'READY_WITH_LIMITATIONS\n'
        return 6
    fi
    printf 'READY\n'
    return 0
}

verify_paths() {
    V_PREFIX="${NEOSENSE_PREFIX:-/usr/local}"
    V_SYSTEMD_DIR="${NEOSENSE_SYSTEMD_DIR:-/etc/systemd/system}"
    V_GROUP="${NEOSENSE_GROUP:-neosense}"
    V_RUNTIME_DIR="${NEOSENSE_RUNTIME_DIR:-/run/neosense}"
    V_SOCKET="${V_RUNTIME_DIR}/neosensed.sock"
    V_BIN="${V_PREFIX}/bin/neosense"
    V_DAEMON="${V_PREFIX}/bin/neosensed"
    V_UNIT="${V_SYSTEMD_DIR}/neosensed.service"
}

verify_run() {
    verify_paths
    V_CHECK_FAIL=0
    V_CHECK_WARN=0
    V_MANUAL_ELIGIBLE=no
    V_FEEDBACK_AVAILABLE=no
    V_FEEDBACK_TRUSTED=no
    V_IDENTITY_VERIFIED=no
    V_HARDWARE_VALIDATED=no
    V_SERVICE_ACTIVE=no

    log_step "Installation verification"
    verify_binaries
    verify_unit
    verify_permissions
    backend_detect
    backend_report
    verify_runtime
    verify_diagnostics
    printf '%-20s %s\n' "fan control" "$(backend_control_ready && printf eligible || printf unavailable)"
    if [ "${V_FEEDBACK_AVAILABLE}" != "yes" ]; then
        printf '%-20s %s\n' "physical feedback" "unavailable"
    elif [ "${V_FEEDBACK_TRUSTED}" = "yes" ]; then
        printf '%-20s %s\n' "physical feedback" "trusted"
    else
        printf '%-20s %s\n' "physical feedback" "available, untrusted"
    fi
    printf '%-20s %s\n' "physical identity" "${V_IDENTITY_VERIFIED}"

    local final status
    set +e
    final="$(verify_final_status)"
    status=$?
    set -e
    printf '\n'
    verify_info "result" "${final}"
    verify_info "checks" "failed ${V_CHECK_FAIL} / warnings ${V_CHECK_WARN}"
    if [ "${final}" = "READY_WITH_LIMITATIONS" ]; then
        log_warn "Software and backend are operational; independent RPM/airflow calibration and physical mapping remain unverified."
        log_warn "Operator-attested CPU/GPU actuator response is documented in docs/VALIDATION_RESULTS.md."
    elif [ "${final}" = "CONTROL_NOT_ELIGIBLE" ]; then
        log_warn "Software is installed, but current hardware policy does not permit manual control."
        log_warn "Run: neosense diagnose"
    elif [ "${final}" = "BACKEND_UNAVAILABLE" ]; then
        log_error "NeoSense cannot control fans until a supported NBFC backend is operational."
    elif [ "${final}" = "INSTALLATION_INCOMPLETE" ]; then
        log_error "Installation is incomplete; repair before relying on fan control."
    fi
    return "${status}"
}
