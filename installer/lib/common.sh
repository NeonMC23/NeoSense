#!/usr/bin/env bash
# Shared helpers for the NeoSense installer. Sourced by neosense.sh.

NEOSENSE_DRY_RUN="${NEOSENSE_DRY_RUN:-0}"
NEOSENSE_ASSUME_YES="${NEOSENSE_ASSUME_YES:-0}"
NEOSENSE_NONINTERACTIVE="${NEOSENSE_NONINTERACTIVE:-0}"

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_RESET=$'\033[0m'
    C_CYAN=$'\033[36m'
    C_GREEN=$'\033[32m'
    C_YELLOW=$'\033[33m'
    C_RED=$'\033[31m'
    C_DIM=$'\033[2m'
else
    C_RESET=''; C_CYAN=''; C_GREEN=''; C_YELLOW=''; C_RED=''; C_DIM=''
fi

log_info() { printf '%b[INFO]%b %s\n' "${C_CYAN}" "${C_RESET}" "$*"; }
log_ok() { printf '%b[OK]%b %s\n' "${C_GREEN}" "${C_RESET}" "$*"; }
log_warn() { printf '%b[WARN]%b %s\n' "${C_YELLOW}" "${C_RESET}" "$*" >&2; }
log_error() { printf '%b[ERROR]%b %s\n' "${C_RED}" "${C_RESET}" "$*" >&2; }
log_step() { printf '\n%b==>%b %s\n' "${C_CYAN}" "${C_RESET}" "$*"; }
log_dim() { printf '%b%s%b\n' "${C_DIM}" "$*" "${C_RESET}"; }

# Boolean switches are consumed with `[ ... -eq 1 ]`; a non-numeric value such
# as "true" would silently skip dry-run and root checks. Reject it instead.
validate_boolean() {
    case "$2" in
        0|1) ;;
        *)
            log_error "Invalid ${1} value '$2': expected 0 or 1."
            exit 2
            ;;
    esac
}

validate_boolean NEOSENSE_DRY_RUN "${NEOSENSE_DRY_RUN}"
validate_boolean NEOSENSE_ASSUME_YES "${NEOSENSE_ASSUME_YES}"
validate_boolean NEOSENSE_NONINTERACTIVE "${NEOSENSE_NONINTERACTIVE}"

die() {
    log_error "$*"
    exit 1
}

usage_error() {
    log_error "$*"
    exit 2
}

have() { command -v "$1" >/dev/null 2>&1; }

run() {
    if [ "${NEOSENSE_DRY_RUN}" -eq 1 ]; then
        printf '+'
        printf ' %q' "$@"
        printf '\n'
        return 0
    fi
    "$@"
}

is_root() { [ "$(id -u)" -eq 0 ]; }

require_root() {
    if ! is_root; then
        die "This operation changes the system. Re-run with: sudo $0 $*"
    fi
}

confirm() {
    local prompt="$1"
    if [ "${NEOSENSE_ASSUME_YES}" -eq 1 ]; then
        return 0
    fi
    if [ "${NEOSENSE_NONINTERACTIVE}" -eq 1 ] || [ ! -t 0 ]; then
        log_error "Confirmation required: ${prompt}"
        log_error "Re-run with --yes to authorize this operation non-interactively."
        return 1
    fi
    local answer
    printf '%s [y/N] ' "${prompt}"
    read -r answer || return 1
    case "${answer}" in
        y|Y|yes|YES) return 0 ;;
        *) return 1 ;;
    esac
}

version_ge() {
    local have="$1" want="$2"
    [ -n "${have}" ] || return 1
    [ -n "${want}" ] || return 0
    [ "$(printf '%s\n%s\n' "${want}" "${have}" | sort -V | head -n1)" = "${want}" ]
}

command_version() {
    local command="$1"
    local base="${command##*/}"
    case "${base}" in
        rustc|cargo) "${command}" --version 2>/dev/null | awk '{print $2}' | head -n1 ;;
        systemctl) "${command}" --version 2>/dev/null | awk 'NR==1 {print $2}' ;;
        nbfc) "${command}" --version 2>/dev/null | awk '{print $2}' | head -n1 ;;
        cc|gcc|clang) "${command}" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -n1 ;;
        bash) "${command}" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -n1 ;;
        awk|gawk|grep) "${command}" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -n1 ;;
        tar) "${command}" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -n1 ;;
        getent|groupadd|ld|gzip|sed|install|mktemp|stat|sha256sum)
            "${command}" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+(\.[0-9]+)?' | head -n1 ;;
        *) "${command}" --version 2>/dev/null | awk 'NR==1 {print $1}' ;;
    esac
}

path_mode() {
    stat -c '%a' "$1" 2>/dev/null || printf 'unknown'
}

path_owner_group() {
    stat -c '%U:%G' "$1" 2>/dev/null || printf 'unknown'
}

unit_active() {
    [ "${NEOSENSE_SKIP_SYSTEMCTL:-0}" -eq 1 ] && return 1
    systemctl is-active --quiet "$1" 2>/dev/null
}

