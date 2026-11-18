#!/usr/bin/env bash
# Command-line surface for the unified NeoSense installer.

NEOSENSE_COMMAND=""
NEOSENSE_NO_RESTART=0
NEOSENSE_NO_START=0
NEOSENSE_SKIP_DEPS=0
NEOSENSE_SOFTWARE_ONLY=0
NEOSENSE_INSTALL_BACKEND=0
NEOSENSE_BACKEND_PACKAGE=""
NEOSENSE_FORCE_PREBUILT=0
NEOSENSE_FORCE_SOURCE=0
BUILD_MODE=""

installer_usage() {
    cat <<'USAGE'
NeoSense installer and diagnostics

Usage:
  sudo ./neosense.sh [install] [options]
  sudo ./neosense.sh upgrade [options]
  sudo ./neosense.sh repair [options]
       ./neosense.sh verify
       ./neosense.sh diagnose
       ./neosense.sh deps
       ./neosense.sh detect
  sudo ./neosense.sh uninstall [--yes] [--dry-run]

Commands:
  install       Detect the host, prepare dependencies, install NeoSense
  upgrade       Same as install; preserves profiles and validation evidence
  repair        Re-run the software installation and verification
  verify        Read-only installation and capability verification
  diagnose      Alias of verify with the diagnostic report
  deps          Show the dependency matrix and current status
  detect        Show distribution, architecture, systemd and hardware detection
  uninstall     Remove NeoSense software; NBFC and user data are preserved

Options:
  --yes                 Authorize listed package and software installation
  --dry-run             Print privileged actions without performing them
  --no-restart          Do not restart an already-running supervisor
  --no-start            Do not enable or start the supervisor
  --skip-deps           Do not install missing packages (still validates)
  --software-only       Install NeoSense without a ready fan backend
  --install-backend     Allow installation of a distro NBFC package
  --backend-package=N   Install this specific NBFC package
  --prebuilt            Require bin/neosense and bin/neosensed
  --source              Require a source build from Cargo.toml

Environment overrides (advanced/test use):
  NEOSENSE_PREFIX, NEOSENSE_SYSTEMD_DIR, NEOSENSE_GROUP,
  NEOSENSE_RUNTIME_DIR, NEOSENSE_STATE_DIR, NEOSENSE_OS_RELEASE_FILE,
  NEOSENSE_PM, NEOSENSE_ARCH, NEOSENSE_SYSTEMD, NEOSENSE_DMI_DIR.

Source builds are performed by install, upgrade and repair when bin/ is absent
or --source is used. There is no separate build command.

Safety:
  The installer never stops another cooling controller, never changes NBFC
  configuration, and never writes fan state. A missing or incompatible backend
  is reported rather than guessed. Use --software-only to install the software
  without starting fan control.
USAGE
}

parse_args() {
    while [ "$#" -gt 0 ]; do
        case "$1" in
            install|upgrade|repair|verify|diagnose|deps|detect|uninstall)
                [ -z "${NEOSENSE_COMMAND}" ] || usage_error "Only one command may be specified."
                NEOSENSE_COMMAND="$1"
                ;;
            --yes|-y) NEOSENSE_ASSUME_YES=1 ;;
            --dry-run) NEOSENSE_DRY_RUN=1 ;;
            --no-restart) NEOSENSE_NO_RESTART=1 ;;
            --no-start) NEOSENSE_NO_START=1 ;;
            --skip-deps) NEOSENSE_SKIP_DEPS=1 ;;
            --software-only) NEOSENSE_SOFTWARE_ONLY=1 ;;
            --install-backend) NEOSENSE_INSTALL_BACKEND=1 ;;
            --backend-package=*) NEOSENSE_BACKEND_PACKAGE="${1#*=}" ;;
            --prebuilt) NEOSENSE_FORCE_PREBUILT=1 ;;
            --source) NEOSENSE_FORCE_SOURCE=1 ;;
            --prefix=*) export NEOSENSE_PREFIX="${1#*=}" ;;
            --systemd-dir=*) export NEOSENSE_SYSTEMD_DIR="${1#*=}" ;;
            --group=*) export NEOSENSE_GROUP="${1#*=}" ;;
            --runtime-dir=*) export NEOSENSE_RUNTIME_DIR="${1#*=}" ;;
            --state-dir=*) export NEOSENSE_STATE_DIR="${1#*=}" ;;
            -h|--help|help) installer_usage; exit 0 ;;
            *) usage_error "Unknown option or command: $1 (use --help)" ;;
        esac
        shift
    done
    [ -n "${NEOSENSE_COMMAND}" ] || NEOSENSE_COMMAND="install"
    if [ ! -t 0 ] && [ "${NEOSENSE_ASSUME_YES}" -ne 1 ] && [ "${NEOSENSE_DRY_RUN}" -ne 1 ]; then
        NEOSENSE_NONINTERACTIVE=1
    fi
}

resolve_build_mode() {
    if [ "${NEOSENSE_FORCE_PREBUILT}" -eq 1 ] && [ "${NEOSENSE_FORCE_SOURCE}" -eq 1 ]; then
        usage_error "--prebuilt and --source are mutually exclusive."
    fi
    if [ "${NEOSENSE_FORCE_SOURCE}" -eq 1 ]; then
        [ -f "${ROOT_DIR}/Cargo.toml" ] || die "--source requires a Cargo.toml in ${ROOT_DIR}."
        BUILD_MODE="source"
        return 0
    fi
    if [ -x "${ROOT_DIR}/bin/neosense" ] && [ -x "${ROOT_DIR}/bin/neosensed" ]; then
        BUILD_MODE="prebuilt"
        return 0
    fi
    if [ "${NEOSENSE_FORCE_PREBUILT}" -eq 1 ]; then
        die "--prebuilt requires bin/neosense and bin/neosensed in ${ROOT_DIR}."
    fi
    if [ -f "${ROOT_DIR}/Cargo.toml" ]; then
        BUILD_MODE="source"
        return 0
    fi
    die "No prebuilt binaries and no Cargo.toml found in ${ROOT_DIR}."
}

preflight_install_host() {
    detect_all
    log_step "Host detection"
    detect_report
    if ! install_supported; then
        die "Installation is not supported on this distribution, architecture or init system. Read-only diagnostics are still available: ./neosense.sh diagnose"
    fi
}

install_missing_dependencies() {
    local missing
    deps_check
    missing="$(deps_missing_required)"
    if [ "${missing}" -eq 0 ]; then
        log_ok "Required dependencies are present."
        return 0
    fi
    deps_report
    if [ "${NEOSENSE_SKIP_DEPS}" -eq 1 ]; then
        die "Missing dependencies and --skip-deps was requested."
    fi
    if [ "${NEOSENSE_NONINTERACTIVE}" -eq 1 ] && [ "${NEOSENSE_ASSUME_YES}" -ne 1 ]; then
        die "Missing dependencies require --yes in non-interactive mode."
    fi
    deps_install_missing
}

prepare_backend_or_exit() {
    local prepare_requested="${NEOSENSE_INSTALL_BACKEND}"
    [ -n "${NEOSENSE_BACKEND_PACKAGE}" ] && prepare_requested=1
    if backend_prepare "${prepare_requested}" "${NEOSENSE_BACKEND_PACKAGE}"; then
        return 0
    fi
    if [ "${NEOSENSE_SOFTWARE_ONLY}" -eq 1 ]; then
        log_warn "Continuing with software only; the supervisor will not be started."
        return 0
    fi
    case "${BACKEND_STATE}" in
        conflict)
            log_warn "A competing fan controller is active. NeoSense software will be installed but fan control will remain disabled."
            NEOSENSE_SOFTWARE_ONLY=1
            return 0
            ;;
        missing|unavailable|incompatible)
            log_error "Installation stopped before changing NeoSense software: the fan-control backend is not ready."
            log_error "Resolve the backend issue above, or use --software-only to install the software without fan control."
            exit 7
            ;;
    esac
    exit 7
}

start_supervisor_if_ready() {
    [ "${NEOSENSE_DRY_RUN}" -eq 1 ] && {
        log_dim "Dry-run: supervisor start is not executed."
        return 0
    }
    [ "${NEOSENSE_SOFTWARE_ONLY}" -eq 0 ] || return 0
    [ "${NEOSENSE_NO_START}" -eq 0 ] || return 0
    backend_control_ready || return 0
    [ "${NEOSENSE_SKIP_SYSTEMCTL:-0}" -eq 0 ] || return 0
    command -v systemctl >/dev/null 2>&1 || return 0
    if systemctl is-active --quiet neosensed 2>/dev/null; then
        return 0
    fi
    log_info "Enabling the supervisor so fan control is available after installation."
    if ! run systemctl enable --now neosensed; then
        log_error "The supervisor could not be started. Installation remains incomplete."
        return 1
    fi
    return 0
}

run_install() {
    if [ "${NEOSENSE_DRY_RUN}" -eq 0 ]; then
        require_root
        if [ "${NEOSENSE_NONINTERACTIVE}" -eq 1 ] && [ "${NEOSENSE_ASSUME_YES}" -ne 1 ]; then
            die "Non-interactive installation requires --yes."
        fi
    fi
    preflight_install_host
    resolve_build_mode
    log_step "Dependency check"
    deps_init
    log_step "Fan-control backend"
    prepare_backend_or_exit
    log_step "Dependency check"
    install_missing_dependencies
    log_step "Install NeoSense"
    local -a install_args=()
    [ "${NEOSENSE_DRY_RUN}" -eq 1 ] && install_args+=(--dry-run)
    if [ "${NEOSENSE_NO_RESTART}" -eq 1 ] || [ "${NEOSENSE_SOFTWARE_ONLY}" -eq 1 ]; then
        install_args+=(--no-restart)
    fi
    "${ROOT_DIR}/install.sh" "${install_args[@]}"
    start_supervisor_if_ready || true
    log_step "Post-install verification"
    verify_run
}

run_repair() {
    run_install
}

run_verify() {
    detect_all
    backend_detect
    verify_run
}

run_deps() {
    detect_all
    resolve_build_mode
    deps_init
    log_step "Host"
    detect_report
    log_step "Dependencies (${BUILD_MODE} installation)"
    deps_report
    if [ "$(deps_missing_required)" -gt 0 ]; then
        return 1
    fi
    return 0
}

run_detect() {
    detect_all
    detect_report
}

run_uninstall() {
    if [ "${NEOSENSE_DRY_RUN}" -eq 0 ]; then
        require_root
        if [ "${NEOSENSE_NONINTERACTIVE}" -eq 1 ] && [ "${NEOSENSE_ASSUME_YES}" -ne 1 ]; then
            die "Non-interactive uninstall requires --yes."
        fi
    fi
    confirm "Uninstall NeoSense? Custom profiles and validation evidence will be preserved; NBFC will not be modified." || exit 1
    local -a args=()
    [ "${NEOSENSE_DRY_RUN}" -eq 1 ] && args+=(--dry-run)
    "${ROOT_DIR}/uninstall.sh" "${args[@]}"
    log_ok "Uninstall completed. NBFC and its configuration were not modified."
}

installer_main() {
    parse_args "$@"
    case "${NEOSENSE_COMMAND}" in
        install|upgrade|repair) run_install ;;
        verify|diagnose) run_verify ;;
        deps) run_deps ;;
        detect) run_detect ;;
        uninstall) run_uninstall ;;
        *) usage_error "Unknown command: ${NEOSENSE_COMMAND}" ;;
    esac
}
