#!/usr/bin/env bash
# NeoSense installer — build the Rust binaries and integrate with systemd.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

COLOR_RESET='\033[0m'
COLOR_BOLD='\033[1m'
COLOR_GREEN='\033[32m'
COLOR_RED='\033[31m'
COLOR_CYAN='\033[36m'

log_info() { printf '%b[INFO]%b %s\n' "${COLOR_CYAN}" "${COLOR_RESET}" "$1"; }
log_success() { printf '%b[OK]%b %s\n' "${COLOR_GREEN}" "${COLOR_RESET}" "$1"; }
log_error() { printf '%b[ERREUR]%b %s\n' "${COLOR_RED}" "${COLOR_RESET}" "$1" >&2; }

if [ "$(id -u)" -ne 0 ]; then
    log_error "Exécutez ce script avec sudo : sudo ./install.sh"
    exit 1
fi

CARGO_BIN="$(command -v cargo || true)"
if [ -z "${CARGO_BIN}" ] && [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
    CARGO_BIN="$(sudo -u "${SUDO_USER}" -H bash -lc 'command -v cargo' 2>/dev/null || true)"
fi
if [ -z "${CARGO_BIN}" ]; then
    log_error "Rust/Cargo est requis pour installer depuis les sources."
    log_error "Installez rustup ou utilisez un paquet précompilé."
    exit 1
fi

log_info "Compilation de NeoSense (release)…"
if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
    sudo -u "${SUDO_USER}" -H bash -lc "cd '${SCRIPT_DIR}' && '${CARGO_BIN}' build --release --locked"
else
    "${CARGO_BIN}" build --release --locked
fi

for binary in neosense neosensed; do
    if [ ! -x "target/release/${binary}" ]; then
        log_error "Binaire manquant : target/release/${binary}"
        exit 1
    fi
    install -Dm755 "target/release/${binary}" "/usr/local/bin/${binary}"
    log_success "/usr/local/bin/${binary}"
done

if [ -f packaging/neosensed.service ]; then
    install -Dm644 packaging/neosensed.service /etc/systemd/system/neosensed.service
    log_success "/etc/systemd/system/neosensed.service"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl daemon-reload
    fi
fi

VERSION="$(/usr/local/bin/neosense --version 2>/dev/null | head -n 1 || true)"
log_success "NeoSense installé : ${VERSION:-version inconnue}"
printf '\n'
printf '%bProchaines étapes :%b\n' "${COLOR_BOLD}" "${COLOR_RESET}"
printf '  neosense status\n'
printf '  neosense diagnose\n'
printf '  sudo systemctl enable --now neosensed   # superviseur (facultatif mais recommandé)\n'
printf '\n'
printf "Le superviseur ne démarre pas automatiquement. Aucun réglage de ventilateur n'a été modifié.\n"
