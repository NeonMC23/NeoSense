#!/usr/bin/env bash
# NeoSense uninstaller — stop the supervisor, remove NeoSense-owned files and
# remove the empty service group only when it is safe to do so.
set -euo pipefail

PREFIX="${NEOSENSE_PREFIX:-/usr/local}"
SYSTEMD_DIR="${NEOSENSE_SYSTEMD_DIR:-/etc/systemd/system}"
GROUP="${NEOSENSE_GROUP:-neosense}"
RUNTIME_DIR="${NEOSENSE_RUNTIME_DIR:-/run/neosense}"
DRY_RUN=0

for arg in "$@"; do
    case "${arg}" in
        --dry-run) DRY_RUN=1 ;;
        --prefix=*) PREFIX="${arg#*=}" ;;
        --systemd-dir=*) SYSTEMD_DIR="${arg#*=}" ;;
        --group=*) GROUP="${arg#*=}" ;;
        --runtime-dir=*) RUNTIME_DIR="${arg#*=}" ;;
        *)
            printf 'Option inconnue: %s\n' "${arg}" >&2
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
log_error() { printf '%b[ERREUR]%b %s\n' "${COLOR_RED}" "${COLOR_RESET}" "$1" >&2; }

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
    log_error "Nom de groupe invalide: ${GROUP}"
    exit 2
fi
if [[ "${PREFIX}" != /* ]] || [[ "${SYSTEMD_DIR}" != /* ]] || [[ "${RUNTIME_DIR}" != /* ]] || [ "${RUNTIME_DIR}" = "/" ]; then
    log_error "Chemins invalides : PREFIX, SYSTEMD_DIR et RUNTIME_DIR doivent être absolus et sûrs."
    exit 2
fi
case "${PREFIX}${SYSTEMD_DIR}${RUNTIME_DIR}" in
    *'#'*|*'&'*) log_error "Les chemins ne doivent pas contenir # ou &."; exit 2 ;;
esac

if [ "${DRY_RUN}" -ne 1 ] && [ "$(id -u)" -ne 0 ]; then
    log_error "Exécutez ce script avec sudo : sudo ./uninstall.sh"
    exit 1
fi

if [ "${DRY_RUN}" -eq 1 ]; then
    log_info "Mode dry-run : aucune modification du système."
fi

if [ "${DRY_RUN}" -ne 1 ] && command -v systemctl >/dev/null 2>&1 && [ -f "${SYSTEMD_DIR}/neosensed.service" ]; then
    log_info "Arrêt du superviseur NeoSense (retour auto via ExecStopPost)…"
    systemctl disable --now neosensed 2>/dev/null || true
fi

run rm -f "${SYSTEMD_DIR}/neosensed.service"
run rm -f "${PREFIX}/bin/neosense" "${PREFIX}/bin/neosensed"
# Nettoyage de l'ancienne installation Bash (v1.x), sans toucher à NBFC.
run rm -rf "${PREFIX}/share/neosense"
run rm -rf "${RUNTIME_DIR}"

if [ "${DRY_RUN}" -ne 1 ] && command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
fi

if getent group "${GROUP}" >/dev/null 2>&1; then
    gid="$(getent group "${GROUP}" | cut -d: -f3)"
    members="$(getent group "${GROUP}" | cut -d: -f4)"
    primary_users="$(getent passwd | awk -F: -v gid="${gid}" '$4 == gid {print $1}')"
    if [ -z "${members}" ] && [ -z "${primary_users}" ]; then
        log_info "Suppression du groupe système vide : ${GROUP}"
        if ! run groupdel "${GROUP}"; then
            log_error "Le groupe ${GROUP} n'a pas pu être supprimé; il est conservé."
        fi
    else
        log_info "Groupe ${GROUP} conservé (membres ou usage primaire détectés)."
    fi
fi

log_success "NeoSense a été désinstallé."
printf "%bNote :%b nbfc_service et sa configuration n'ont pas été modifiés.\n" "${COLOR_BOLD}" "${COLOR_RESET}"
