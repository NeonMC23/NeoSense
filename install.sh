#!/usr/bin/env bash
# NeoSense installer — build the Rust binaries, create the service group and
# install the systemd unit. It never starts the supervisor automatically.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

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
if [[ "${PREFIX}" != /* ]] || [[ "${SYSTEMD_DIR}" != /* ]] || [[ "${RUNTIME_DIR}" != /* ]]; then
    log_error "PREFIX, SYSTEMD_DIR et RUNTIME_DIR doivent être des chemins absolus."
    exit 2
fi
case "${PREFIX}${SYSTEMD_DIR}${RUNTIME_DIR}" in
    *'#'*|*'&'*) log_error "Les chemins ne doivent pas contenir # ou &."; exit 2 ;;
esac

if [ "${DRY_RUN}" -ne 1 ] && [ "$(id -u)" -ne 0 ]; then
    log_error "Exécutez ce script avec sudo : sudo ./install.sh"
    exit 1
fi

if [ "${DRY_RUN}" -eq 1 ]; then
    log_info "Mode dry-run : aucune modification du système."
else
    if getent group "${GROUP}" >/dev/null 2>&1; then
        log_info "Groupe système déjà présent : ${GROUP}"
    else
        log_info "Création du groupe système : ${GROUP}"
        run groupadd --system "${GROUP}"
    fi
fi

CARGO_BIN="$(command -v cargo || true)"
if [ -z "${CARGO_BIN}" ] && [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
    CARGO_BIN="$(sudo -u "${SUDO_USER}" -H bash -lc 'command -v cargo' 2>/dev/null || true)"
fi

if [ "${DRY_RUN}" -ne 1 ]; then
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
fi

for binary in neosense neosensed; do
    if [ "${DRY_RUN}" -ne 1 ] && [ ! -x "target/release/${binary}" ]; then
        log_error "Binaire manquant : target/release/${binary}"
        exit 1
    fi
    run install -Dm755 "target/release/${binary}" "${PREFIX}/bin/${binary}"
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
    run install -Dm644 "${UNIT_TMP}" "${SYSTEMD_DIR}/neosensed.service"
    log_success "${SYSTEMD_DIR}/neosensed.service"
fi

run install -d -m 0750 -o root -g "${GROUP}" "${RUNTIME_DIR}"

if [ "${DRY_RUN}" -ne 1 ] && command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload
fi

if [ "${DRY_RUN}" -ne 1 ]; then
    VERSION="$("${PREFIX}/bin/neosense" --version 2>/dev/null | head -n 1 || true)"
    log_success "NeoSense installé : ${VERSION:-version inconnue}"
fi

printf '\n'
printf '%bProchaines étapes :%b\n' "${COLOR_BOLD}" "${COLOR_RESET}"
printf '  neosense status\n'
printf '  neosense diagnose\n'
if [ -n "${SUDO_USER:-}" ] && [ "${SUDO_USER}" != "root" ]; then
    printf '  sudo usermod -aG %s %s\n' "${GROUP}" "${SUDO_USER}"
    printf '  # puis ouvrez une nouvelle session pour appliquer le groupe\n'
fi
printf '  sudo systemctl enable --now neosensed   # superviseur (facultatif mais recommandé)\n'
printf '\n'
printf "Le superviseur ne démarre pas automatiquement. Aucun réglage de ventilateur n'a été modifié.\n"
