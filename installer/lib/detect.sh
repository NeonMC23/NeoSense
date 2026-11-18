#!/usr/bin/env bash
# Host, distribution, architecture and systemd detection.

detect_os() {
    local file="${NEOSENSE_OS_RELEASE_FILE:-/etc/os-release}"
    DISTRO_ID=""
    DISTRO_LIKE=""
    DISTRO_NAME=""
    DISTRO_VERSION=""
    DISTRO_FAMILY="unknown"

    if [ -f "${file}" ]; then
        # shellcheck disable=SC1090,SC1091
        . "${file}"
        DISTRO_ID="${ID:-}"
        DISTRO_LIKE="${ID_LIKE:-}"
        DISTRO_NAME="${PRETTY_NAME:-${NAME:-unknown}}"
        DISTRO_VERSION="${VERSION_ID:-}"
    else
        DISTRO_NAME="unknown"
    fi

    local tokens=" ${DISTRO_ID} ${DISTRO_LIKE} "
    case "${tokens}" in
        *" nobara "*|*" fedora "*|*" rhel "*|*" centos "*|*" rocky "*|*" alma "*)
            DISTRO_FAMILY="fedora" ;;
        *" debian "*|*" ubuntu "*|*" linuxmint "*|*" pop "*)
            DISTRO_FAMILY="debian" ;;
        *" arch "*|*" manjaro "*|*" endeavouros "*|*" cachyos "*)
            DISTRO_FAMILY="arch" ;;
        *" opensuse "*|*" opensuse-leap "*|*" opensuse-tumbleweed "*|*" sles "*|*" suse "*)
            DISTRO_FAMILY="suse" ;;
        *) DISTRO_FAMILY="unknown" ;;
    esac
}

detect_arch() {
    ARCH="${NEOSENSE_ARCH:-$(uname -m 2>/dev/null || printf unknown)}"
    case "${ARCH}" in
        x86_64|amd64|aarch64|arm64) ARCH_SUPPORTED=1 ;;
        *) ARCH_SUPPORTED=0 ;;
    esac
}

detect_systemd() {
    SYSTEMD_AVAILABLE=0
    SYSTEMD_VERSION=""
    if [ "${NEOSENSE_SYSTEMD:-}" = "0" ]; then
        return 0
    fi
    if [ "${NEOSENSE_SYSTEMD:-}" = "1" ] || { have systemctl && [ -d /run/systemd/system ]; }; then
        SYSTEMD_AVAILABLE=1
        SYSTEMD_VERSION="${NEOSENSE_SYSTEMD_VERSION:-$(systemctl --version 2>/dev/null | awk 'NR==1 {print $2}')}"
    fi
}

detect_package_manager() {
    PM=""
    # shellcheck disable=SC2034
    PM_FAMILY=""
    if [ -n "${NEOSENSE_PM:-}" ]; then
        PM="${NEOSENSE_PM}"
    else
        case "${DISTRO_FAMILY}" in
            fedora)
                if have dnf; then PM="dnf"
                elif have dnf5; then PM="dnf5"
                fi
                ;;
            debian) have apt-get && PM="apt" || true ;;
            arch) have pacman && PM="pacman" || true ;;
            suse) have zypper && PM="zypper" || true ;;
        esac
        if [ -z "${PM}" ]; then
            for candidate in dnf dnf5 apt-get pacman zypper; do
                if have "${candidate}"; then
                    PM="${candidate}"
                    break
                fi
            done
        fi
    fi
    case "${PM}" in
        dnf|dnf5) PM_FAMILY="dnf" ;;
        apt|apt-get) PM="apt"; PM_FAMILY="apt" ;;
        pacman) PM_FAMILY="pacman" ;;
        zypper) PM_FAMILY="zypper" ;;
        *) PM_FAMILY="unknown" ;;
    esac
    export PM_FAMILY
}

detect_host() {
    local dmi="${NEOSENSE_DMI_DIR:-/sys/class/dmi/id}"
    HOST_VENDOR="$(cat "${dmi}/sys_vendor" 2>/dev/null | tr -d '\n' || true)"
    HOST_MODEL="$(cat "${dmi}/product_name" 2>/dev/null | tr -d '\n' || true)"
    HOST_BOARD="$(cat "${dmi}/board_name" 2>/dev/null | tr -d '\n' || true)"
    HOST_BIOS="$(cat "${dmi}/bios_version" 2>/dev/null | tr -d '\n' || true)"
    KERNEL="${NEOSENSE_KERNEL:-$(uname -r 2>/dev/null || printf unknown)}"
    HOST_LABEL="${HOST_VENDOR:-Machine} ${HOST_MODEL:-unknown}"
}

detect_all() {
    detect_os
    detect_arch
    detect_systemd
    detect_package_manager
    detect_host
}

install_supported() {
    [ "${ARCH_SUPPORTED}" -eq 1 ] || return 1
    [ "${SYSTEMD_AVAILABLE}" -eq 1 ] || return 1
    [ "${DISTRO_FAMILY}" != "unknown" ] || return 1
    return 0
}

detect_report() {
    printf '%-20s %s\n' "distribution" "${DISTRO_NAME:-unknown}"
    printf '%-20s %s\n' "id" "${DISTRO_ID:-unknown}"
    printf '%-20s %s\n' "version" "${DISTRO_VERSION:-unknown}"
    printf '%-20s %s\n' "family" "${DISTRO_FAMILY}"
    printf '%-20s %s\n' "architecture" "${ARCH}"
    printf '%-20s %s\n' "kernel" "${KERNEL}"
    printf '%-20s %s\n' "systemd" "${SYSTEMD_AVAILABLE} (${SYSTEMD_VERSION:-unknown})"
    printf '%-20s %s\n' "package manager" "${PM:-none}"
    printf '%-20s %s\n' "root" "$(is_root && printf yes || printf no)"
    printf '%-20s %s\n' "host" "${HOST_LABEL}"
    printf '%-20s %s\n' "board" "${HOST_BOARD:-unknown}"
    printf '%-20s %s\n' "bios" "${HOST_BIOS:-unknown}"
    if install_supported; then
        printf '%-20s %s\n' "installation" "supported path"
    else
        printf '%-20s %s\n' "installation" "not supported on this host"
    fi
}
