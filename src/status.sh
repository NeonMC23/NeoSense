#!/usr/bin/env bash
# NeoSense - Status Monitor
# Retrieves active status, temperatures, fan speeds, and prints a gorgeous dashboard

show_status() {
    # 1. Determine daemon state
    local service_active="Stopped"
    local service_color="$COLOR_RED"
    if systemctl is-active nbfc_service >/dev/null 2>&1; then
        service_active="Running"
        service_color="$COLOR_GREEN"
    fi

    local autostart_status="Disabled"
    local autostart_color="$COLOR_RED"
    if systemctl is-enabled nbfc_service >/dev/null 2>&1; then
        autostart_status="Enabled"
        autostart_color="$COLOR_GREEN"
    fi

    # 2. Configuration name
    local current_cfg
    current_cfg=$(get_current_config)
    if [ "$current_cfg" = "None" ] || [ -z "$current_cfg" ]; then
        current_cfg="Not configured"
    fi

    # 3. NBFC properties parsing (default to placeholder values)
    local fan_count=0
    local nbfc_temp="N/A"
    
    local fan_1_name="CPU"
    local fan_1_auto="True"
    local fan_1_current="0"
    local fan_1_target="0"
    
    local fan_2_name="GPU"
    local fan_2_auto="True"
    local fan_2_current="0"
    local fan_2_target="0"

    # Only query nbfc status if the service is active
    if [ "$service_active" = "Running" ]; then
        local status_out
        status_out=$(nbfc status -a 2>/dev/null)

        # Parse output using awk to convert block values to sequential keys
        while IFS=: read -r key val; do
            case "$key" in
                CONFIG) [ -n "$val" ] && current_cfg="$val" ;;
                TEMP) nbfc_temp="$val" ;;
                FAN_COUNT) fan_count="$val" ;;
                FAN_1_NAME) fan_1_name="$val" ;;
                FAN_1_AUTO) fan_1_auto="$val" ;;
                FAN_1_CURRENT) fan_1_current="$val" ;;
                FAN_1_TARGET) fan_1_target="$val" ;;
                FAN_2_NAME) fan_2_name="$val" ;;
                FAN_2_AUTO) fan_2_auto="$val" ;;
                FAN_2_CURRENT) fan_2_current="$val" ;;
                FAN_2_TARGET) fan_2_target="$val" ;;
            esac
        done <<EOF
$(echo "$status_out" | awk -F':' '
/Selected config name/ { gsub(/^[ \t]+|[ \t]+$/, "", $2); print "CONFIG:" $2 }
/Temperature/ { gsub(/^[ \t]+|[ \t]+$/, "", $2); print "TEMP:" $2 }
/Fan display name/ {
    fan_count++
    gsub(/^[ \t]+|[ \t]+$/, "", $2)
    print "FAN_" fan_count "_NAME:" $2
}
/Auto control enabled/ {
    gsub(/^[ \t]+|[ \t]+$/, "", $2)
    print "FAN_" fan_count "_AUTO:" $2
}
/Current fan speed/ {
    gsub(/^[ \t]+|[ \t]+$/, "", $2)
    print "FAN_" fan_count "_CURRENT:" $2
}
/Target fan speed/ {
    gsub(/^[ \t]+|[ \t]+$/, "", $2)
    print "FAN_" fan_count "_TARGET:" $2
}
END {
    print "FAN_COUNT:" fan_count
}
')
EOF
    fi

    # 4. Mode determination (Auto or Manual)
    # If any active fan is not in auto mode, we consider the mode Manual
    local mode_status="Auto"
    local mode_color="$COLOR_GREEN"
    if [ "$service_active" = "Running" ]; then
        if [ "$fan_1_auto" = "False" ] || [ "$fan_2_auto" = "False" ]; then
            mode_status="Manual"
            mode_color="$COLOR_YELLOW"
        fi
    else
        mode_status="N/A"
        mode_color="$COLOR_RED"
    fi

    # 5. Temperature discovery
    local cpu_temp
    local gpu_temp
    cpu_temp=$(get_cpu_temp)
    gpu_temp=$(get_gpu_temp)

    # CPU Temperature Fallback
    if [ -z "$cpu_temp" ] || [ "$cpu_temp" = "N/A" ]; then
        if [ -n "$nbfc_temp" ] && [ "$nbfc_temp" != "N/A" ]; then
            cpu_temp=$(echo "$nbfc_temp" | cut -d'.' -f1)
        else
            cpu_temp="N/A"
        fi
    fi

    # GPU Temperature Fallback
    if [ -z "$gpu_temp" ] || [ "$gpu_temp" = "N/A" ]; then
        gpu_temp="N/A"
    fi

    # 6. Formatting Speeds
    local cpu_current="N/A"
    local cpu_target="N/A"
    local gpu_current="N/A"
    local gpu_target="N/A"

    if [ "$service_active" = "Running" ]; then
        cpu_current=$(echo "$fan_1_current" | cut -d'.' -f1)
        cpu_target=$(echo "$fan_1_target" | cut -d'.' -f1)

        if [ "$fan_count" -ge 2 ]; then
            gpu_current=$(echo "$fan_2_current" | cut -d'.' -f1)
            gpu_target=$(echo "$fan_2_target" | cut -d'.' -f1)
        else
            gpu_current="N/A"
            gpu_target="N/A"
        fi
    fi

    # Prepare Display Labels with units
    local cpu_temp_disp="${cpu_temp}"
    [ "$cpu_temp" != "N/A" ] && cpu_temp_disp="${cpu_temp}°C"
    
    local gpu_temp_disp="${gpu_temp}"
    [ "$gpu_temp" != "N/A" ] && gpu_temp_disp="${gpu_temp}°C"

    local cpu_current_disp="${cpu_current}"
    [ "$cpu_current" != "N/A" ] && cpu_current_disp="${cpu_current}%"
    
    local cpu_target_disp="${cpu_target}"
    [ "$cpu_target" != "N/A" ] && cpu_target_disp="${cpu_target}%"

    local gpu_current_disp="${gpu_current}"
    [ "$gpu_current" != "N/A" ] && gpu_current_disp="${gpu_current}%"
    
    local gpu_target_disp="${gpu_target}"
    [ "$gpu_target" != "N/A" ] && gpu_target_disp="${gpu_target}%"

    # Render clean output
    echo -e "${COLOR_BOLD_CYAN}NeoSense v1.0${COLOR_RESET}"
    echo ""
    echo -e "${COLOR_BOLD}Configuration :${COLOR_RESET}"
    echo -e " ${COLOR_CYAN}${current_cfg}${COLOR_RESET}"
    echo ""
    echo -e "${COLOR_BOLD}Service :${COLOR_RESET}"
    echo -e " ${service_color}${service_active}${COLOR_RESET}"
    echo ""
    echo -e "${COLOR_BOLD}Autostart :${COLOR_RESET}"
    echo -e " ${autostart_color}${autostart_status}${COLOR_RESET}"
    echo ""
    echo -e "${COLOR_BOLD}Mode :${COLOR_RESET}"
    echo -e " ${mode_color}${mode_status}${COLOR_RESET}"
    echo ""
    echo -e "${COLOR_BOLD_WHITE}CPU${COLOR_RESET}"
    echo -e "  Temp      ${COLOR_CYAN}${cpu_temp_disp}${COLOR_RESET}"
    echo -e "  Current   ${COLOR_GREEN}${cpu_current_disp}${COLOR_RESET}"
    echo -e "  Target    ${COLOR_YELLOW}${cpu_target_disp}${COLOR_RESET}"
    echo ""
    echo -e "${COLOR_BOLD_WHITE}GPU${COLOR_RESET}"
    echo -e "  Temp      ${COLOR_CYAN}${gpu_temp_disp}${COLOR_RESET}"
    echo -e "  Current   ${COLOR_GREEN}${gpu_current_disp}${COLOR_RESET}"
    echo -e "  Target    ${COLOR_YELLOW}${gpu_target_disp}${COLOR_RESET}"
}
