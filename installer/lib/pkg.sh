#!/usr/bin/env bash
# Package-manager adapters. Every privileged operation goes through pkg_install.

pkg_available() {
    local package="$1"
    case "${PM_FAMILY}" in
        dnf)
            if [ "${PM}" = "dnf5" ]; then
                dnf5 -q info "${package}" >/dev/null 2>&1
            else
                dnf -q info "${package}" >/dev/null 2>&1
            fi
            ;;
        apt) apt-cache show "${package}" >/dev/null 2>&1 ;;
        pacman) pacman -Si "${package}" >/dev/null 2>&1 ;;
        zypper) zypper --non-interactive info "${package}" >/dev/null 2>&1 ;;
        *) return 1 ;;
    esac
}

pkg_install() {
    [ "$#" -gt 0 ] || return 0
    if [ "${NEOSENSE_DRY_RUN}" -eq 1 ]; then
        printf '+'
        printf ' %q' "${PM}" "$@"
        printf '\n'
        return 0
    fi
    if ! is_root; then
        log_error "Package installation requires root."
        return 1
    fi
    case "${PM_FAMILY}" in
        dnf)
            if [ "${PM}" = "dnf5" ]; then
                dnf5 install -y --setopt=install_weak_deps=False "$@"
            else
                dnf install -y --setopt=install_weak_deps=False "$@"
            fi
            ;;
        apt)
            DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$@"
            ;;
        pacman)
            pacman -S --needed --noconfirm "$@"
            ;;
        zypper)
            zypper --non-interactive install --no-recommends "$@"
            ;;
        *)
            log_error "No supported package manager is available."
            return 1
            ;;
    esac
}

