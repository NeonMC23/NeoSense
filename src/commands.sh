#!/usr/bin/env bash
# NeoSense - Fan Control Commands
# Directly maps NeoSense fan commands to NBFC and manages profile presets

# Put all fans in Auto mode
set_fans_auto() {
    require_root
    log_info "Switching all fans to Automatic mode..."
    if nbfc set -a >/dev/null 2>&1; then
        log_success "All fans successfully set to automatic mode."
    else
        log_error "Failed to set fans to automatic mode. Ensure the NBFC service is running."
        return 1
    fi
}

# Put all fans in Manual mode without changing current speeds
set_fans_manual() {
    require_root
    log_info "Switching all fans to Manual mode (without modifying speeds)..."
    
    local success=0
    # Try setting Fan 0 (CPU) to manual
    if nbfc set -f 0 >/dev/null 2>&1; then
        success=1
    fi
    # Try setting Fan 1 (GPU) to manual
    if nbfc set -f 1 >/dev/null 2>&1; then
        success=$((success + 1))
    fi

    if [ "$success" -gt 0 ]; then
        log_success "Fans switched to manual mode successfully ($success fan(s) detected)."
    else
        # Try setting general auto mode off if individual fans fail
        log_error "Failed to switch fans to manual mode. Ensure the NBFC service is running."
        return 1
    fi
}

# Set CPU fan speed (Fan index 0)
set_cpu_speed() {
    local speed="$1"
    if ! is_valid_speed "$speed"; then
        log_error "Invalid speed value: '$speed'. Speed must be an integer between 0 and 100."
        return 1
    fi

    require_root
    log_info "Setting CPU fan speed to ${speed}%..."
    if nbfc set -f 0 -s "$speed" >/dev/null 2>&1; then
        log_success "CPU fan speed set to ${speed}%."
    else
        log_error "Failed to set CPU fan speed. Check if NBFC service is active."
        return 1
    fi
}

# Set GPU fan speed (Fan index 1)
set_gpu_speed() {
    local speed="$1"
    if ! is_valid_speed "$speed"; then
        log_error "Invalid speed value: '$speed'. Speed must be an integer between 0 and 100."
        return 1
    fi

    require_root
    log_info "Setting GPU fan speed to ${speed}%..."
    if nbfc set -f 1 -s "$speed" >/dev/null 2>&1; then
        log_success "GPU fan speed set to ${speed}%."
    else
        log_error "Failed to set GPU fan speed. Check if NBFC service is active and if your system has a GPU fan (index 1)."
        return 1
    fi
}

# Set both CPU and GPU speeds
set_both_speed() {
    local speed="$1"
    if ! is_valid_speed "$speed"; then
        log_error "Invalid speed value: '$speed'. Speed must be an integer between 0 and 100."
        return 1
    fi

    require_root
    log_info "Setting both CPU and GPU fan speeds to ${speed}%..."
    
    local success=0
    if nbfc set -f 0 -s "$speed" >/dev/null 2>&1; then
        success=1
    fi
    if nbfc set -f 1 -s "$speed" >/dev/null 2>&1; then
        success=$((success + 1))
    fi

    if [ "$success" -eq 2 ]; then
        log_success "Both CPU and GPU fan speeds set to ${speed}%."
    elif [ "$success" -eq 1 ]; then
        log_success "CPU fan speed set to ${speed}% (GPU fan not available or failed)."
    else
        log_error "Failed to set fan speeds. Ensure the NBFC service is running."
        return 1
    fi
}

# Profile preset functions
set_profile_turbo() {
    log_info "Activating Profile: ${COLOR_BOLD_RED}TURBO${COLOR_RESET} (CPU: 100%, GPU: 100%)"
    set_both_speed 100
}

set_profile_performance() {
    log_info "Activating Profile: ${COLOR_BOLD_YELLOW}PERFORMANCE${COLOR_RESET} (CPU: 80%, GPU: 80%)"
    set_both_speed 80
}

set_profile_balanced() {
    log_info "Activating Profile: ${COLOR_BOLD_GREEN}BALANCED${COLOR_RESET} (CPU: 60%, GPU: 60%)"
    set_both_speed 60
}

set_profile_silent() {
    log_info "Activating Profile: ${COLOR_BOLD_CYAN}SILENT${COLOR_RESET} (CPU: 25%, GPU: 25%)"
    set_both_speed 25
}
