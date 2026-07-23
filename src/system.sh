#!/usr/bin/env bash
# NeoSense - System & Daemon Functions
# Handles temperature polling and systemd daemon management for nbfc_service

# Retrieve CPU Temperature using sysfs (fast, direct) or lm-sensors
get_cpu_temp() {
    # 1. Search sysfs for CPU sensors (Intel coretemp, AMD k10temp, zenpower)
    for hwmon in /sys/class/hwmon/hwmon*; do
        if [ -f "$hwmon/name" ]; then
            local name
            name=$(cat "$hwmon/name")
            if [ "$name" = "coretemp" ] || [ "$name" = "k10temp" ] || [ "$name" = "zenpower" ]; then
                # Look for temp1_input or any other temp input
                for temp_file in "$hwmon"/temp*_input; do
                    if [ -f "$temp_file" ]; then
                        local temp_raw
                        temp_raw=$(cat "$temp_file")
                        # sysfs reports temp in millidegrees C, convert to C
                        echo $((temp_raw / 1000))
                        return 0
                    fi
                done
            fi
        fi
    done

    # 2. Fallback to sensors command
    if command -v sensors >/dev/null 2>&1; then
        local temp
        temp=$(sensors 2>/dev/null | grep -E '(Core 0|Tctl|Package id 0)' | head -n 1 | grep -oE '[+-][0-9.]+' | head -n 1 | cut -d'.' -f1 | tr -d '+-')
        if [ -n "$temp" ]; then
            echo "$temp"
            return 0
        fi
    fi

    # Return empty to allow fallback to nbfc's reported temperature
    echo ""
}

# Retrieve GPU Temperature using sysfs, nvidia-smi, or lm-sensors
get_gpu_temp() {
    # 1. Search sysfs for GPU sensors (amdgpu, nouveau, radeon)
    for hwmon in /sys/class/hwmon/hwmon*; do
        if [ -f "$hwmon/name" ]; then
            local name
            name=$(cat "$hwmon/name")
            if [ "$name" = "amdgpu" ] || [ "$name" = "nouveau" ] || [ "$name" = "radeon" ]; then
                for temp_file in "$hwmon"/temp*_input; do
                    if [ -f "$temp_file" ]; then
                        local temp_raw
                        temp_raw=$(cat "$temp_file")
                        echo $((temp_raw / 1000))
                        return 0
                    fi
                done
            fi
        fi
    done

    # 2. Try nvidia-smi if using NVIDIA proprietary drivers
    if command -v nvidia-smi >/dev/null 2>&1; then
        local temp
        temp=$(nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>/dev/null)
        if [ -n "$temp" ] && is_integer "$temp"; then
            echo "$temp"
            return 0
        fi
    fi

    # 3. Fallback to sensors command (searching for GPU temperature keys)
    if command -v sensors >/dev/null 2>&1; then
        local temp
        temp=$(sensors 2>/dev/null | grep -E '(edge|junction|mem|gpu|temp1)' | head -n 1 | grep -oE '[+-][0-9.]+' | head -n 1 | cut -d'.' -f1 | tr -d '+-')
        if [ -n "$temp" ]; then
            echo "$temp"
            return 0
        fi
    fi

    echo ""
}

# Enable and start the NBFC service
daemon_enable() {
    require_root
    log_info "Enabling and starting NBFC service (nbfc_service)..."
    if systemctl enable --now nbfc_service; then
        log_success "NBFC service enabled and started successfully."
    else
        log_error "Failed to enable and start NBFC service. Please check if nbfc-linux is properly installed."
        return 1
    fi
}

# Disable and stop the NBFC service
daemon_disable() {
    require_root
    log_info "Disabling and stopping NBFC service (nbfc_service)..."
    if systemctl disable --now nbfc_service; then
        log_success "NBFC service disabled and stopped successfully."
    else
        log_error "Failed to disable and stop NBFC service."
        return 1
    fi
}

# Restart the NBFC service
daemon_restart() {
    require_root
    log_info "Restarting NBFC service (nbfc_service)..."
    if systemctl restart nbfc_service; then
        log_success "NBFC service restarted successfully."
    else
        log_error "Failed to restart NBFC service."
        return 1
    fi
}

# Display the active status and autostart configuration of nbfc_service
daemon_status() {
    local active_str
    if systemctl is-active nbfc_service >/dev/null 2>&1; then
        active_str="${COLOR_GREEN}Running${COLOR_RESET}"
    else
        active_str="${COLOR_RED}Stopped${COLOR_RESET}"
    fi

    local enabled_str
    if systemctl is-enabled nbfc_service >/dev/null 2>&1; then
        enabled_str="${COLOR_GREEN}Enabled${COLOR_RESET}"
    else
        enabled_str="${COLOR_RED}Disabled${COLOR_RESET}"
    fi

    echo -e "$active_str"
    echo -e "Autostart :"
    echo -e "$enabled_str"
}
