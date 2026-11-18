#!/usr/bin/env bash
# Dependency metadata and checks. Markdown-level metadata lives here once.

DEP_ORDER="bash systemd grep coreutils tar gzip sed gawk getent shadow-utils rustc cargo cc ld nvidia-smi acpiexec"
declare -A DEP_KIND DEP_REQUIRED DEP_MIN DEP_PURPOSE DEP_DNF DEP_APT DEP_PACMAN DEP_ZYPPER DEP_STATUS DEP_VERSION

dep_define() {
    local name="$1" kind="$2" required="$3" min="$4" purpose="$5"
    local dnf="$6" apt="$7" pacman="$8" zypper="$9"
    DEP_KIND["${name}"]="${kind}"
    DEP_REQUIRED["${name}"]="${required}"
    DEP_MIN["${name}"]="${min}"
    DEP_PURPOSE["${name}"]="${purpose}"
    DEP_DNF["${name}"]="${dnf}"
    DEP_APT["${name}"]="${apt}"
    DEP_PACMAN["${name}"]="${pacman}"
    DEP_ZYPPER["${name}"]="${zypper}"
}

deps_define_all() {
    dep_define bash runtime mandatory 4.0 "installer scripting and arrays" \
        bash bash bash bash
    dep_define grep install mandatory 0 "status, configuration and log parsing" \
        grep grep grep grep
    dep_define systemd runtime mandatory 245 "systemd service, watchdog and sandboxing" \
        systemd systemd systemd systemd
    dep_define coreutils install mandatory 0 "install, mktemp, stat, sha256sum and basic file tools" \
        coreutils coreutils coreutils coreutils
    dep_define tar install mandatory 0 "release archive extraction and configuration backups" \
        tar tar tar tar
    dep_define gzip install mandatory 0 "compressed release archives and backups" \
        gzip gzip gzip gzip
    dep_define sed install mandatory 0 "systemd unit path substitution" \
        sed sed sed sed
    dep_define gawk install mandatory 0 "version and status parsing" \
        gawk gawk gawk gawk
    dep_define getent runtime mandatory 0 "group and user resolution" \
        glibc-common libc-bin glibc glibc
    dep_define shadow-utils runtime mandatory 0 "system group creation" \
        shadow-utils passwd shadow shadow
    dep_define rustc build mandatory 1.85 "Rust compiler for a source build" \
        rust cargo rustc rust
    dep_define cargo build mandatory 1.85 "Cargo for a locked source build" \
        cargo cargo rust cargo
    dep_define cc build mandatory 0 "C linker driver used by rustc" \
        gcc build-essential gcc gcc
    dep_define ld build mandatory 0 "linker used by rustc" \
        binutils binutils binutils binutils
    dep_define nvidia-smi optional optional 0 "independent NVIDIA GPU temperature guard" \
        "" "" "" ""
    dep_define acpiexec optional optional 0 "optional NBFC rate-config analysis" \
        acpica-tools acpica-tools acpica acpica
}

dep_packages() {
    local name="$1"
    case "${PM_FAMILY}" in
        dnf) printf '%s\n' "${DEP_DNF[${name}]}" ;;
        apt) printf '%s\n' "${DEP_APT[${name}]}" ;;
        pacman) printf '%s\n' "${DEP_PACMAN[${name}]}" ;;
        zypper) printf '%s\n' "${DEP_ZYPPER[${name}]}" ;;
        *) printf '' ;;
    esac
}

dep_version() {
    local name="$1"
    case "${name}" in
        systemd) printf '%s' "${SYSTEMD_VERSION}" ;;
        coreutils)
            if have install; then command_version install; fi
            ;;
        shadow-utils)
            if have groupadd; then command_version groupadd; fi
            ;;
        gawk)
            if have gawk; then command_version gawk
            elif have awk; then command_version awk
            fi
            ;;
        cc)
            local compiler
            compiler="$(command -v cc || command -v gcc || command -v clang || true)"
            [ -n "${compiler}" ] && command_version "${compiler}"
            ;;
        ld)
            if have ld; then command_version ld; fi
            ;;
        *) if have "${name}"; then command_version "${name}"; fi ;;
    esac
}

dep_present() {
    local name="$1"
    if [ "${NEOSENSE_TEST_MISSING_DEP:-}" = "${name}" ]; then
        return 1
    fi
    case "${name}" in
        systemd) [ "${SYSTEMD_AVAILABLE}" -eq 1 ] ;;
        coreutils) have install && have sha256sum && have mktemp && have stat ;;
        shadow-utils) have groupadd ;;
        gawk) have awk || have gawk ;;
        cc) have cc || have gcc || have clang ;;
        ld) have ld ;;
        *) have "${name}" ;;
    esac
}

deps_check() {
    local name kind required min version
    # shellcheck disable=SC2086
    for name in ${DEP_ORDER}; do
        kind="${DEP_KIND[${name}]}"
        required="${DEP_REQUIRED[${name}]}"
        min="${DEP_MIN[${name}]}"
        if [ "${kind}" = "build" ] && [ "${BUILD_MODE:-prebuilt}" != "source" ]; then
            DEP_STATUS["${name}"]="NOT_NEEDED"
            DEP_VERSION["${name}"]=""
            continue
        fi
        version="$(dep_version "${name}" || true)"
        DEP_VERSION["${name}"]="${version}"
        if [ "${NEOSENSE_TEST_OLD_DEP:-}" = "${name}" ]; then
            DEP_STATUS["${name}"]="OLD"
        elif dep_present "${name}"; then
            if [ -n "${min}" ] && [ "${min}" != "0" ] && ! version_ge "${version}" "${min}"; then
                DEP_STATUS["${name}"]="OLD"
            else
                DEP_STATUS["${name}"]="OK"
            fi
        elif [ "${required}" = "optional" ]; then
            DEP_STATUS["${name}"]="OPTIONAL_MISSING"
        else
            DEP_STATUS["${name}"]="MISSING"
        fi
    done
}

deps_missing_required() {
    local name count=0
    # shellcheck disable=SC2086
    for name in ${DEP_ORDER}; do
        case "${DEP_STATUS[${name}]:-}" in
            MISSING|OLD) count=$((count + 1)) ;;
        esac
    done
    printf '%s' "${count}"
}

deps_report() {
    local name kind required min status version packages detail
    printf '%-14s %-9s %-9s %-9s %-9s %s\n' "DEPENDENCY" "KIND" "REQUIRED" "STATUS" "VERSION" "PURPOSE"
    # shellcheck disable=SC2086
    for name in ${DEP_ORDER}; do
        kind="${DEP_KIND[${name}]}"
        required="${DEP_REQUIRED[${name}]}"
        status="${DEP_STATUS[${name}]:-UNKNOWN}"
        version="${DEP_VERSION[${name}]:-}"
        min="${DEP_MIN[${name}]}"
        detail="${DEP_PURPOSE[${name}]}"
        packages="$(dep_packages "${name}")"
        [ "${status}" = "OLD" ] && detail="${detail} (minimum ${min})"
        if [ "${status}" = "MISSING" ] || [ "${status}" = "OLD" ]; then
            if [ -n "${packages}" ]; then
                detail="${detail}; package: ${packages}"
            else
                detail="${detail}; no distro package mapping"
            fi
        fi
        printf '%-14s %-9s %-9s %-9s %-9s %s\n' \
            "${name}" "${kind}" "${required}" "${status}" "${version:-—}" "${detail}"
    done
    if [ -n "${PM}" ]; then
        printf '\nPackage manager: %s (%s)\n' "${PM}" "${PM_FAMILY}"
    else
        printf '\nNo supported package manager detected; automatic installation is unavailable.\n'
    fi
}

deps_install_missing() {
    local name status packages package failed=0
    local -a pending=() pending_names=()
    # shellcheck disable=SC2086
    for name in ${DEP_ORDER}; do
        status="${DEP_STATUS[${name}]:-}"
        case "${status}" in
            MISSING|OLD)
                if [ "${DEP_KIND[${name}]}" = "build" ] && [ "${BUILD_MODE:-prebuilt}" != "source" ]; then
                    continue
                fi
                packages="$(dep_packages "${name}")"
                if [ -z "${packages}" ]; then
                    log_warn "No package mapping for ${name}; manual installation is required."
                    failed=$((failed + 1))
                    continue
                fi
                pending_names+=("${name}")
                local -a package_list=()
                read -r -a package_list <<< "${packages//,/ }"
                for package in "${package_list[@]}"; do
                    pending+=("${package}")
                done
                ;;
        esac
    done
    if [ "${#pending[@]}" -eq 0 ]; then
        return 0
    fi
    if [ -z "${PM}" ]; then
        log_error "Missing dependencies (${pending_names[*]}) and no supported package manager."
        return 1
    fi
    local unavailable=0
    for package in "${pending[@]}"; do
        if ! pkg_available "${package}"; then
            log_warn "Package not available in configured repositories: ${package}"
            unavailable=1
        fi
    done
    if [ "${unavailable}" -ne 0 ]; then
        log_error "Cannot resolve every missing dependency from the configured repositories."
        return 1
    fi
    if ! confirm "Install missing dependencies with ${PM}: ${pending_names[*]}?"; then
        log_error "Dependency installation was not authorized."
        return 1
    fi
    log_info "Installing packages: ${pending[*]}"
    pkg_install "${pending[@]}" || {
        log_error "Package installation failed."
        return 1
    }
    deps_check
    for name in "${pending_names[@]}"; do
        case "${DEP_STATUS[${name}]:-}" in
            MISSING|OLD)
                log_error "${name} is still not suitable after installation."
                failed=$((failed + 1))
                ;;
        esac
    done
    if [ "${failed}" -gt 0 ]; then
        log_error "Dependency resolution is incomplete."
        log_error "For a Rust toolchain older than 1.85, install rustup from https://rustup.rs manually or use a prebuilt release archive."
        return 1
    fi
    return 0
}

deps_init() {
    deps_define_all
    deps_check
}
