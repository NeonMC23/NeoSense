#!/usr/bin/env bash
# NeoSense - Configuration Management
# Handles listing, reading, and applying NBFC configs

# Internal helper to retrieve the name of the active config
get_current_config() {
    local status_out
    status_out=$(nbfc status 2>/dev/null)
    local config
    config=$(echo "$status_out" | grep -i "Selected config name" | cut -d':' -f2- | xargs)
    
    if [ -n "$config" ]; then
        echo "$config"
    else
        # Fallback: check config file if the daemon is stopped
        if [ -f "/etc/nbfc/nbfc.json" ]; then
            config=$(grep -i '"SelectedConfigName"' /etc/nbfc/nbfc.json | cut -d':' -f2 | tr -d '", \t')
        fi
        if [ -z "$config" ]; then
            echo "None"
        else
            echo "$config"
        fi
    fi
}

# Print the current configuration
show_current_config() {
    local cfg
    cfg=$(get_current_config)
    if [ "$cfg" = "None" ]; then
        log_info "No configuration is currently active."
    else
        echo -e "${COLOR_CYAN}${cfg}${COLOR_RESET}"
    fi
}

# List all available NBFC profiles
list_configs() {
    local config_dir="/usr/share/nbfc/configs"
    if [ ! -d "$config_dir" ]; then
        # Try NBFC direct command listing as fallback
        if command -v nbfc >/dev/null 2>&1; then
            log_info "Listing available profiles via NBFC command..."
            nbfc config -l
            return 0
        else
            log_error "Config directory '$config_dir' not found and 'nbfc' CLI is unreachable."
            return 1
        fi
    fi

    log_info "Available NBFC configurations:"
    echo ""

    local files
    files=$(find "$config_dir" -maxdepth 1 -type f \( -name "*.json" -o -name "*.xml" \) -printf "%f\n" 2>/dev/null | sed -E 's/\.(json|xml)$//' | sort -f)

    if [ -z "$files" ]; then
        # If no json/xml found, output all files in that directory
        files=$(ls -1 "$config_dir" | sort -f)
    fi

    if [ -z "$files" ]; then
        log_warning "No configurations found in $config_dir."
        return 1
    fi

    # Format listing in multiple columns if possible, fallback to plain output
    if command -v column >/dev/null 2>&1; then
        echo "$files" | column -c 80
    else
        echo "$files"
    fi
}

# Set and apply a profile name
use_config() {
    local config_name="$1"
    if [ -z "$config_name" ]; then
        log_error "Missing configuration name."
        log_info "Usage: neosense config use \"<profile_name>\""
        return 1
    fi

    require_root

    log_info "Applying configuration: \"$config_name\"..."
    
    # Apply using nbfc config
    if nbfc config -a "$config_name" >/dev/null 2>&1; then
        log_success "Configuration \"$config_name\" applied successfully."
        log_info "Restarting NBFC service..."
        if systemctl restart nbfc_service >/dev/null 2>&1; then
            log_success "NBFC service restarted. NeoSense is active with \"$config_name\"!"
        else
            log_warning "Failed to restart systemd service 'nbfc_service'. Try starting it manually."
        fi
    else
        log_error "Failed to apply config. Please check if \"$config_name\" is a valid profile name."
        log_info "Check available configs with: neosense config list"
        return 1
    fi
}
