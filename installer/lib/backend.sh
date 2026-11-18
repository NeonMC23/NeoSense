#!/usr/bin/env bash
# Fan-control backend detection and safe preparation. This module never writes
# fan state, never changes NBFC configuration and never starts or stops a
# backend service.

BACKEND_STATE="unknown"
BACKEND_KIND="none"
BACKEND_VERSION=""
BACKEND_SERVICE_ACTIVE="no"
BACKEND_SOCKET_EXISTS="no"
BACKEND_CONFIG_EXISTS="no"
BACKEND_SOCKET_MODE="unknown"
BACKEND_CONFLICT=""
BACKEND_CONTROL="no"
BACKEND_NOTES=""

backend_nbfc_bin() {
    if [ -n "${NEOSENSE_NBFC_BIN:-}" ]; then
        printf '%s' "${NEOSENSE_NBFC_BIN}"
    else
        command -v nbfc 2>/dev/null || true
    fi
}

backend_reset() {
    BACKEND_STATE="unknown"
    BACKEND_KIND="none"
    BACKEND_VERSION=""
    BACKEND_SERVICE_ACTIVE="no"
    BACKEND_SOCKET_EXISTS="no"
    BACKEND_CONFIG_EXISTS="no"
    BACKEND_SOCKET_MODE="unknown"
    BACKEND_CONFLICT=""
    BACKEND_CONTROL="no"
    BACKEND_NOTES=""
}

backend_conflicts() {
    local service
    BACKEND_CONFLICT=""
    for service in fancontrol thinkfan coolercontrol; do
        if unit_active "${service}" 2>/dev/null; then
            BACKEND_CONFLICT="${BACKEND_CONFLICT}${BACKEND_CONFLICT:+ }${service}"
        fi
    done
}

backend_detect() {
    backend_reset
    local bin socket config service_active=0
    bin="$(backend_nbfc_bin)"
    if [ -n "${NEOSENSE_NBFC_BIN:-}" ] && [ ! -x "${NEOSENSE_NBFC_BIN}" ]; then
        bin=""
    fi
    socket="${NEOSENSE_NBFC_SOCKET:-/run/nbfc_service.socket}"
    config="${NEOSENSE_NBFC_CONFIG:-/etc/nbfc/nbfc.json}"

    backend_conflicts
    if [ "${NEOSENSE_NBFC_SOCKET_EXISTS:-}" = "1" ]; then
        BACKEND_SOCKET_EXISTS="yes"
        BACKEND_SOCKET_MODE="${NEOSENSE_NBFC_SOCKET_MODE:-600}"
    elif [ -S "${socket}" ]; then
        BACKEND_SOCKET_EXISTS="yes"
        BACKEND_SOCKET_MODE="$(path_mode "${socket}")"
    fi
    if [ "${NEOSENSE_NBFC_CONFIG_EXISTS:-}" = "1" ] || [ -f "${config}" ]; then
        BACKEND_CONFIG_EXISTS="yes"
    fi
    if [ "${NEOSENSE_NBFC_SERVICE_ACTIVE:-}" = "1" ]; then
        service_active=1
    elif [ "${SYSTEMD_AVAILABLE}" -eq 1 ] && [ "${NEOSENSE_SKIP_SYSTEMCTL:-0}" -ne 1 ]; then
        systemctl is-active --quiet nbfc_service 2>/dev/null && service_active=1
    fi
    BACKEND_SERVICE_ACTIVE="$([ "${service_active}" -eq 1 ] && printf yes || printf no)"

    if [ -z "${bin}" ]; then
        BACKEND_STATE="missing"
        BACKEND_NOTES="nbfc executable not found"
        return 0
    fi
    BACKEND_KIND="nbfc"
    BACKEND_VERSION="${NEOSENSE_NBFC_VERSION:-$("${bin}" --version 2>/dev/null | awk '{print $2}' | head -n1)}"
    if [ -z "${BACKEND_VERSION}" ]; then
        BACKEND_STATE="incompatible"
        BACKEND_NOTES="nbfc does not report a version"
        return 0
    fi
    if [ -n "${BACKEND_CONFLICT}" ]; then
        BACKEND_STATE="conflict"
        BACKEND_NOTES="another fan-control service is active: ${BACKEND_CONFLICT}"
        return 0
    fi
    if [ "${service_active}" -ne 1 ]; then
        BACKEND_STATE="unavailable"
        BACKEND_NOTES="nbfc_service is not active"
        return 0
    fi
    if ! version_ge "${BACKEND_VERSION}" "0.5.3"; then
        BACKEND_STATE="incompatible"
        BACKEND_NOTES="NBFC ${BACKEND_VERSION} is older than the supported 0.5.3/0.5.4 baseline"
        return 0
    fi
    if ! version_ge "${BACKEND_VERSION}" "0.5.4"; then
        BACKEND_STATE="read_only"
        BACKEND_CONTROL="limited"
        BACKEND_NOTES="NBFC ${BACKEND_VERSION} supports read-only and automatic mode; protected manual control requires 0.5.4 or newer"
        return 0
    fi
    BACKEND_STATE="ready"
    BACKEND_CONTROL="yes"
    if [ "${BACKEND_SOCKET_EXISTS}" != "yes" ]; then
        BACKEND_STATE="unavailable"
        BACKEND_CONTROL="no"
        BACKEND_NOTES="nbfc_service is active but its socket is missing"
    elif [ "${BACKEND_SOCKET_MODE}" != "600" ] && [ "${BACKEND_SOCKET_MODE}" != "660" ]; then
        BACKEND_NOTES="NBFC ${BACKEND_VERSION} is ready; socket mode ${BACKEND_SOCKET_MODE} is not access-controlled and manual control may be blocked"
    fi
    return 0
}

backend_report() {
    printf '%-20s %s\n' "backend" "${BACKEND_KIND} ${BACKEND_VERSION:-—}"
    printf '%-20s %s\n' "state" "${BACKEND_STATE}"
    printf '%-20s %s\n' "service active" "${BACKEND_SERVICE_ACTIVE}"
    printf '%-20s %s\n' "socket" "$([ "${BACKEND_SOCKET_EXISTS}" = yes ] && printf present || printf missing) (${BACKEND_SOCKET_MODE})"
    printf '%-20s %s\n' "configuration" "${BACKEND_CONFIG_EXISTS}"
    printf '%-20s %s\n' "manual control" "${BACKEND_CONTROL}"
    [ -n "${BACKEND_NOTES}" ] && printf '%-20s %s\n' "notes" "${BACKEND_NOTES}"
    [ -n "${BACKEND_CONFLICT}" ] && printf '%-20s %s\n' "conflicts" "${BACKEND_CONFLICT}"
    return 0
}

backend_control_ready() {
    [ "${BACKEND_STATE}" = "ready" ]
}

backend_backup_config() {
    local config_dir="${NEOSENSE_NBFC_CONFIG_DIR:-/etc/nbfc}"
    [ -d "${config_dir}" ] || return 0
    local backup_root="${NEOSENSE_STATE_DIR:-/var/lib/neosense}/installer-backups"
    local stamp
    stamp="$(date +%Y%m%d-%H%M%S)"
    if [ "${NEOSENSE_DRY_RUN}" -eq 1 ]; then
        printf '+ tar -C %q -czf %q %q\n' "$(dirname "${config_dir}")" "${backup_root}/nbfc-${stamp}.tar.gz" "$(basename "${config_dir}")"
        return 0
    fi
    mkdir -p "${backup_root}"
    tar -C "$(dirname "${config_dir}")" -czf "${backup_root}/nbfc-${stamp}.tar.gz" "$(basename "${config_dir}")"
    log_info "NBFC configuration backup: ${backup_root}/nbfc-${stamp}.tar.gz"
}

backend_instructions() {
    cat <<'INSTRUCTIONS'
NBFC is required for NeoSense fan control and was not found in a supported state.
This installer will not download or execute an unverified NBFC installer.

Safe options:
  1. Install the NBFC package from your distribution, if one is provided:
       Fedora/Nobara:  sudo dnf install nbfc
       Debian/Ubuntu:  sudo apt install nbfc
       Arch:           install the nbfc-linux package from your configured source
       openSUSE:       sudo zypper install nbfc
  2. Or install a verified NBFC 0.5.4+ build from its upstream project, then rerun:
       sudo ./neosense.sh install
  3. You can still install only the NeoSense software with:
       sudo ./neosense.sh install --software-only

NBFC 0.5.3 is read-only for NeoSense (automatic mode is available, protected
manual control is not). NBFC 0.5.4 or newer is required for manual control.
INSTRUCTIONS
}

backend_install_package() {
    local package="${1:-}"
    [ -n "${PM}" ] || {
        log_error "No supported package manager; cannot install NBFC automatically."
        return 1
    }
    if [ -n "${package}" ]; then
        pkg_available "${package}" || {
            log_error "Package ${package} is not available in the configured repositories."
            return 1
        }
        confirm "Install NBFC backend package '${package}'?" || return 1
        backend_backup_config
        pkg_install "${package}" || return 1
        backend_detect
        return 0
    fi
    local candidate
    for candidate in nbfc nbfc-linux; do
        if pkg_available "${candidate}"; then
            confirm "Install NBFC backend package '${candidate}'?" || return 1
            backend_backup_config
            pkg_install "${candidate}" || return 1
            backend_detect
            return 0
        fi
    done
    return 1
}

backend_prepare() {
    local requested="${1:-0}" package="${2:-}"
    backend_detect
    case "${BACKEND_STATE}" in
        ready|read_only)
            log_ok "Fan-control backend: nbfc ${BACKEND_VERSION} (${BACKEND_STATE})."
            return 0
            ;;
        missing)
            if [ "${requested}" != "1" ] && [ -t 0 ] && [ "${NEOSENSE_ASSUME_YES}" -eq 0 ] && [ -n "${PM}" ]; then
                if confirm "NBFC is missing. Try installing an NBFC package from ${PM} now?"; then
                    requested=1
                fi
            fi
            if [ "${requested}" = "1" ]; then
                log_info "NBFC is missing; attempting a supported package installation."
                if backend_install_package "${package}"; then
                    backend_report
                    case "${BACKEND_STATE}" in
                        ready|read_only) return 0 ;;
                        *) return 1 ;;
                    esac
                fi
            fi
            backend_instructions
            return 1
            ;;
        incompatible|unavailable|conflict)
            log_warn "Fan-control backend is not ready: ${BACKEND_STATE} — ${BACKEND_NOTES}"
            if [ -n "${BACKEND_CONFLICT}" ]; then
                log_warn "Do not stop another controller automatically. Disable ${BACKEND_CONFLICT} manually only if you understand the conflict."
            fi
            log_warn "The installer will not modify or restart NBFC or competing services."
            return 1
            ;;
        *)
            log_error "Unknown backend state: ${BACKEND_STATE}"
            return 1
            ;;
    esac
}
