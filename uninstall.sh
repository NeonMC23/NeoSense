#!/usr/bin/env bash
# NeoSense uninstaller — stop the supervisor, return NBFC to auto, remove files.
set -euo pipefail

COLOR_RESET='\033[0m'
COLOR_BOLD='\033[1m'
COLOR_GREEN='\033[32m'
COLOR_RED='\033[31m'
COLOR_CYAN='\033[36m'

log_info() { printf '%b[INFO]%b %s\n' "${COLOR_CYAN}" "${COLOR_RESET}" "$1"; }
log_success() { printf '%b[OK]%b %s\n' "${COLOR_GREEN}" "${COLOR_RESET}" "$1"; }
log_error() { printf '%b[ERREUR]%b %s\n' "${COLOR_RED}" "${COLOR_RESET}" "$1" >&2; }

if [ "$(id -u)" -ne 0 ]; then
    log_error "Exécutez ce script avec sudo : sudo ./uninstall.sh"
    exit 1
fi

if command -v systemctl >/dev/null 2>&1; then
    if systemctl list-unit-files neosensed.service >/dev/null 2>&1; then
        log_info "Arrêt du superviseur NeoSense (retour auto via ExecStopPost)…"
        systemctl disable --now neosensed 2>/dev/null || true
    fi
fi

rm -f /etc/systemd/system/neosensed.service
rm -f /usr/local/bin/neosense /usr/local/bin/neosensed
rm -rf /run/neosense
# Nettoyage de l’ancienne installation Bash (v1.x), sans toucher à NBFC.
rm -rf /usr/local/share/neosense

if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
fi

log_success "NeoSense a été désinstallé."
printf "%bNote :%b nbfc_service et sa configuration n'ont pas été modifiés.\n" "${COLOR_BOLD}" "${COLOR_RESET}"
