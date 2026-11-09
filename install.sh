#!/usr/bin/env bash
# NeoSense installer — install prebuilt release binaries or build from source,
# create the service group and install the systemd unit. It never starts the
# supervisor automatically.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

PREFIX="${NEOSENSE_PREFIX:-/usr/local}"
SYSTEMD_DIR="${NEOSENSE_SYSTEMD_DIR:-/etc/systemd/system}"
GROUP="${NEOSENSE_GROUP:-neosense}"
RUNTIME_DIR="${NEOSENSE_RUNTIME_DIR:-/run/neosense}"
STATE_DIR="${NEOSENSE_STATE_DIR:-/var/lib/neosense}"
DRY_RUN=0

for arg in "$@"; do
    case "${arg}" in
        --dry-run) DRY_RUN=1 ;;
        --prefix=*) PREFIX="${arg#*=}" ;;
        --systemd-dir=*) SYSTEMD_DIR="${arg#*=}" ;;
        --group=*) GROUP="${arg#*=}" ;;
        --runtime-dir=*) RUNTIME_DIR="${arg#*=}" ;;
        --state-dir=*) STATE_DIR="${arg#*=}" ;;
        *)
            printf 'Unknown option: %s\n' "${arg}" >&2
            exit 2
            ;;
    esac
done

COLOR_RESET='\033[0m'
COLOR_BOLD='\033[1m'
COLOR_GREEN='\033[32m'
COLOR_RED='\033[31m'
COLOR_CYAN='\033[36m'

log_info() { printf '%b[INFO]%b %s\n' "${COLOR_CYAN}" "${COLOR_RESET}" "$1"; }
log_success() { printf '%b[OK]%b %s\n' "${COLOR_GREEN}" "${COLOR_RESET}" "$1"; }
log_error() { printf '%b[ERROR]%b %s\n' "${COLOR_RED}" "${COLOR_RESET}" "$1" >&2; }

run() {
    if [ "${DRY_RUN}" -eq 1 ]; then
        printf '+'
        printf ' %q' "$@"
        printf '\n'
        return 0
    fi
    "$@"
}

if ! [[ "${GROUP}" =~ ^[a-z_][a-z0-9_-]*$ ]]; then
    log_error "Invalid group name: ${GROUP}"
    exit 2
fi
if [[ "${PREFIX}" != /* ]] || [[ "${SYSTEMD_DIR}" != /* ]] || [[ "${RUNTIME_DIR}" != /* ]] || [[ "${STATE_DIR}" != /* ]]; then
    log_error "PREFIX, SYSTEMD_DIR, RUNTIME_DIR and STATE_DIR must be absolute paths."
    exit 2
fi
case "${PREFIX}${SYSTEMD_DIR}${RUNTIME_DIR}${STATE_DIR}" in
    *'#'*|*'&'*) log_error "Paths must not contain # or &."; exit 2 ;;
esac

if [ "${DRY_RUN}" -ne 1 ] && [ "$(id -u)" -ne 0 ]; then
    log_error "Run this script with sudo: sudo ./install.sh"
    exit 1
fi

if [ "${DRY_RUN}" -eq 1 ]; then
    log_info "Dry-run: no system changes."
else
    if getent group "${GROUP}" >/dev/null 2>&1; then
        log_info "System group already exists: ${GROUP}"
    else
        log_info "Creating system group: ${GROUP}"
        run groupadd --system "${GROUP}"
    fi
fi

PREBUILT_DIR="${SCRIPT_DIR}/bin"
if [ -x "${PREBUILT_DIR}/neosense" ] && [ -x "${PREBUILT_DIR}/neosensed" ]; then
    BINARY_DIR="${PREBUILT_DIR}"
    if [ "${DRY_RUN}" -ne 1 ]; then
        log_info "Using prebuilt release binaries."
    fi
elif [ -f "${SCRIPT_DIR}/Cargo.toml" ]; then
    BINARY_DIR="${SCRIPT_DIR}/target/release"
    CARGO_BIN="$(command -v cargo || true)"
    if [ -z "${CARGO_BIN}" ] && [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
        CARGO_BIN="$(sudo -u "${SUDO_USER}" -H bash -lc 'command -v cargo' 2>/dev/null || true)"
    fi

    if [ "${DRY_RUN}" -ne 1 ]; then
        if [ -z "${CARGO_BIN}" ]; then
            log_error "Rust/Cargo is required for a source install."
            log_error "Install rustup or use the release archive with prebuilt binaries."
            exit 1
        fi

        log_info "Building NeoSense (release)…"
        if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
            sudo -u "${SUDO_USER}" -H bash -lc "cd '${SCRIPT_DIR}' && '${CARGO_BIN}' build --release --locked"
        else
            "${CARGO_BIN}" build --release --locked
        fi
    fi
else
    log_error "No prebuilt binaries and no Cargo.toml found."
    log_error "Use a complete release archive or a source checkout."
    exit 1
fi

BACKUP_DIR=""
OLD_VERSION=""
NEW_VERSION=""

if [ "${DRY_RUN}" -ne 1 ]; then
    for binary in neosense neosensed; do
        if [ ! -x "${BINARY_DIR}/${binary}" ]; then
            log_error "Missing binary: ${BINARY_DIR}/${binary}"
            exit 1
        fi
    done
    NEW_VERSION="$("${BINARY_DIR}/neosense" --version 2>/dev/null | head -n 1 || true)"
    if [ -z "${NEW_VERSION}" ]; then
        log_error "The neosense binary does not report a version."
        exit 1
    fi
    if [ -x "${PREFIX}/bin/neosense" ]; then
        OLD_VERSION="$("${PREFIX}/bin/neosense" --version 2>/dev/null | head -n 1 || true)"
    fi

    BACKUP_DIR="${STATE_DIR}/backups/$(date +%Y%m%d-%H%M%S)-$$"
    mkdir -p "${BACKUP_DIR}/bin" "${BACKUP_DIR}/systemd"
    for binary in neosense neosensed; do
        if [ -f "${PREFIX}/bin/${binary}" ]; then
            cp -a "${PREFIX}/bin/${binary}" "${BACKUP_DIR}/bin/${binary}"
        fi
    done
    if [ -f "${SYSTEMD_DIR}/neosensed.service" ]; then
        cp -a "${SYSTEMD_DIR}/neosensed.service" "${BACKUP_DIR}/systemd/neosensed.service"
    fi
    log_info "Backup created: ${BACKUP_DIR}"
    if [ -n "${OLD_VERSION}" ]; then
        log_info "Upgrading ${OLD_VERSION} -> ${NEW_VERSION}"
    else
        log_info "Fresh install: ${NEW_VERSION}"
    fi
fi

restore_file() {
    local target="$1"
    local backup="$2"
    if [ -f "${backup}" ]; then
        cp -a "${backup}" "${target}"
    else
        rm -f "${target}"
    fi
}

rollback() {
    [ "${DRY_RUN}" -eq 1 ] && return 0
    [ -n "${BACKUP_DIR}" ] || return 0
    log_error "Installation failed; restoring the previous files."
    restore_file "${PREFIX}/bin/neosense" "${BACKUP_DIR}/bin/neosense"
    restore_file "${PREFIX}/bin/neosensed" "${BACKUP_DIR}/bin/neosensed"
    restore_file "${SYSTEMD_DIR}/neosensed.service" "${BACKUP_DIR}/systemd/neosensed.service"
}

for binary in neosense neosensed; do
    if ! run install -Dm755 "${BINARY_DIR}/${binary}" "${PREFIX}/bin/${binary}"; then
        rollback
        exit 1
    fi
    log_success "${PREFIX}/bin/${binary}"
done

UNIT_SOURCE="packaging/neosensed.service"
if [ -f "${UNIT_SOURCE}" ]; then
    UNIT_TMP="$(mktemp)"
    trap 'rm -f "${UNIT_TMP}"' EXIT
    sed \
        -e "s#/usr/local/bin#${PREFIX}/bin#g" \
        -e "s#/run/neosense#${RUNTIME_DIR}#g" \
        -e "s#Group=neosense#Group=${GROUP}#g" \
        -e "s#NEOSENSE_SOCKET_GROUP=neosense#NEOSENSE_SOCKET_GROUP=${GROUP}#g" \
        "${UNIT_SOURCE}" > "${UNIT_TMP}"
    if ! run install -Dm644 "${UNIT_TMP}" "${SYSTEMD_DIR}/neosensed.service"; then
        rollback
        exit 1
    fi
    log_success "${SYSTEMD_DIR}/neosensed.service"
fi

if ! run install -d -m 0750 -o root -g "${GROUP}" "${RUNTIME_DIR}"; then
    rollback
    exit 1
fi

if [ "${DRY_RUN}" -ne 1 ]; then
    INSTALLED_VERSION="$("${PREFIX}/bin/neosense" --version 2>/dev/null | head -n 1 || true)"
    if [ "${INSTALLED_VERSION}" != "${NEW_VERSION}" ]; then
        log_error "Installed version verification failed (expected ${NEW_VERSION}, got ${INSTALLED_VERSION:-none})."
        rollback
        exit 1
    fi
fi

if [ "${DRY_RUN}" -ne 1 ] && [ "${NEOSENSE_SKIP_SYSTEMCTL:-0}" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
fi

if [ "${DRY_RUN}" -ne 1 ]; then
    VERSION="$("${PREFIX}/bin/neosense" --version 2>/dev/null | head -n 1 || true)"
    log_success "NeoSense installed: ${VERSION:-unknown version}"
fi

printf '\n'
printf '%bNext steps:%b\n' "${COLOR_BOLD}" "${COLOR_RESET}"
printf '  neosense status\n'
printf '  neosense diagnose\n'
if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
    printf '  sudo usermod -aG %s %s\n' "${GROUP}" "${SUDO_USER}"
    printf '  # then start a new login session to apply the group\n'
fi
printf '  sudo systemctl enable --now neosensed   # supervisor (optional, recommended)\n'
printf '\n'
printf "The supervisor is not started automatically. No fan setting was changed.\n"
