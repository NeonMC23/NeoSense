#!/usr/bin/env bash
# NeoSense - Utility Functions
# Contains coloring, logging, and validation helpers

# Check if stdout is a TTY and NO_COLOR is not set (standard no-color.org support)
if [ -t 1 ] && [ -z "${NO_COLOR}" ]; then
    COLOR_RESET='\033[0m'
    COLOR_BOLD='\033[1m'
    COLOR_RED='\033[31m'
    COLOR_GREEN='\033[32m'
    COLOR_YELLOW='\033[33m'
    COLOR_BLUE='\033[34m'
    COLOR_MAGENTA='\033[35m'
    COLOR_CYAN='\033[36m'
    COLOR_WHITE='\033[37m'
    COLOR_BOLD_RED='\033[1;31m'
    COLOR_BOLD_GREEN='\033[1;32m'
    COLOR_BOLD_YELLOW='\033[1;33m'
    COLOR_BOLD_CYAN='\033[1;36m'
    COLOR_BOLD_WHITE='\033[1;37m'
else
    COLOR_RESET=''
    COLOR_BOLD=''
    COLOR_RED=''
    COLOR_GREEN=''
    COLOR_YELLOW=''
    COLOR_BLUE=''
    COLOR_MAGENTA=''
    COLOR_CYAN=''
    COLOR_WHITE=''
    COLOR_BOLD_RED=''
    COLOR_BOLD_GREEN=''
    COLOR_BOLD_YELLOW=''
    COLOR_BOLD_CYAN=''
    COLOR_BOLD_WHITE=''
fi

# Logging functions for output consistency
log_info() {
    echo -e "${COLOR_CYAN}[INFO]${COLOR_RESET} $1"
}

log_success() {
    echo -e "${COLOR_GREEN}[SUCCESS]${COLOR_RESET} $1"
}

log_warning() {
    echo -e "${COLOR_YELLOW}[WARNING]${COLOR_RESET} $1"
}

log_error() {
    echo -e "${COLOR_RED}[ERROR]${COLOR_RESET} $1" >&2
}

# Check if running as root (required for most fan control and systemd commands)
is_root() {
    [ "$(id -u)" -eq 0 ]
}

# Restrict commands to root/sudo
require_root() {
    if ! is_root; then
        log_error "This command requires root privileges. Please run with 'sudo neosense ...' or as root."
        exit 1
    fi
}

# Validate if input is a non-negative integer
is_integer() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

# Validate if speed is in 0-100% range
is_valid_speed() {
    local val="$1"
    if is_integer "$val" && [ "$val" -ge 0 ] && [ "$val" -le 100 ]; then
        return 0
    fi
    return 1
}

# Check if NBFC CLI is installed
check_nbfc_installed() {
    if ! command -v nbfc >/dev/null 2>&1; then
        log_error "NBFC (Notebook FanControl) is not installed on this system!"
        log_error "NeoSense is a CLI wrapper and requires NBFC to communicate with the Embedded Controller."
        log_info "Please install nbfc-linux (on Arch: 'yay -S nbfc-linux'; on other systems, install from source)."
        return 1
    fi
    return 0
}
