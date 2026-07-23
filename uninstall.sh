#!/usr/bin/env bash
# NeoSense - Uninstallation Script
# Safely removes NeoSense and its files from the system

set -e

# Color definitions
COLOR_RESET='\033[0m'
COLOR_BOLD='\033[1m'
COLOR_GREEN='\033[32m'
COLOR_RED='\033[31m'
COLOR_CYAN='\033[36m'

log_info() {
    echo -e "${COLOR_CYAN}[INFO]${COLOR_RESET} $1"
}

log_success() {
    echo -e "${COLOR_GREEN}[SUCCESS]${COLOR_RESET} $1"
}

log_error() {
    echo -e "${COLOR_RED}[ERROR]${COLOR_RESET} $1" >&2
}

# Require root privileges to uninstall
if [ "$(id -u)" -ne 0 ]; then
    log_error "Please run this script as root or with sudo:"
    echo -e "  ${COLOR_BOLD}sudo ./uninstall.sh${COLOR_RESET}"
    exit 1
fi

INSTALL_DIR="/usr/local/share/neosense"
BIN_LINK="/usr/local/bin/neosense"

# Remove symlink
if [ -L "${BIN_LINK}" ] || [ -f "${BIN_LINK}" ]; then
    log_info "Removing executable symlink: ${BIN_LINK}..."
    rm -f "${BIN_LINK}"
fi

# Remove installation directory
if [ -d "${INSTALL_DIR}" ]; then
    log_info "Removing files and directory: ${INSTALL_DIR}..."
    rm -rf "${INSTALL_DIR}"
fi

log_success "NeoSense has been successfully uninstalled!"
echo "Thank you for using NeoSense! If you encountered any bugs, please feel free to report them."
