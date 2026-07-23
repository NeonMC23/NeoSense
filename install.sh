#!/usr/bin/env bash
# NeoSense - Installation Script
# Installs NeoSense as a globally accessible tool under /usr/local/bin/

set -e

# Color definitions
COLOR_RESET='\033[0m'
COLOR_BOLD='\033[1m'
COLOR_GREEN='\033[32m'
COLOR_RED='\033[31m'
COLOR_CYAN='\033[36m'
COLOR_BOLD_GREEN='\033[1;32m'

log_info() {
    echo -e "${COLOR_CYAN}[INFO]${COLOR_RESET} $1"
}

log_success() {
    echo -e "${COLOR_GREEN}[SUCCESS]${COLOR_RESET} $1"
}

log_error() {
    echo -e "${COLOR_RED}[ERROR]${COLOR_RESET} $1" >&2
}

# 1. Require root privileges to install
if [ "$(id -u)" -ne 0 ]; then
    log_error "Please run this script as root or with sudo:"
    echo -e "  ${COLOR_BOLD}sudo ./install.sh${COLOR_RESET}"
    exit 1
fi

# 2. Check if NBFC is installed
log_info "Verifying NBFC installation..."
if ! command -v nbfc >/dev/null 2>&1; then
    log_error "NBFC (Notebook FanControl) is not installed on your system!"
    log_error "NeoSense is a wrapper around NBFC and requires it to communicate with your Embedded Controller."
    log_info "Please install nbfc-linux (e.g. from AUR: 'nbfc-linux' on Arch, or from source)."
    log_info "Once installed, run this script again."
    exit 1
fi
log_success "NBFC installation found."

# 3. Destination folders
INSTALL_DIR="/usr/local/share/neosense"
BIN_LINK="/usr/local/bin/neosense"

# 4. Copy files
log_info "Creating directory ${INSTALL_DIR}..."
mkdir -p "${INSTALL_DIR}/src"

log_info "Copying NeoSense files to ${INSTALL_DIR}..."
cp neosense "${INSTALL_DIR}/"
cp src/*.sh "${INSTALL_DIR}/src/"

# 5. Set ownership & permissions
log_info "Setting secure file permissions..."
chmod +x "${INSTALL_DIR}/neosense"
chmod 644 "${INSTALL_DIR}/src"/*.sh

# 6. Create symbolic link in /usr/local/bin
log_info "Creating executable symlink: ${BIN_LINK}..."
rm -f "${BIN_LINK}"
ln -s "${INSTALL_DIR}/neosense" "${BIN_LINK}"

log_success "NeoSense installed successfully!"
echo ""
echo -e "You can now run NeoSense commands globally:"
echo -e "  - ${COLOR_BOLD_GREEN}neosense status${COLOR_RESET}"
echo -e "  - ${COLOR_BOLD_GREEN}neosense --help${COLOR_RESET}"
echo ""
echo -e "Enjoy cooler temperatures! ❄️"
