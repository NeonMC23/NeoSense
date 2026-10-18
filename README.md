 <p align="center">
  <img src="https://img.shields.io/badge/LINUX-SYSTEM%20CONTROL-A3E635?style=for-the-badge&labelColor=171717" alt="Linux system control">
</p>

<h1 align="center">NeoSense</h1>

<p align="center">
  <strong>Your laptop. Your fans. Your control.</strong>
  <br>
  A modern, elegant CLI wrapper for NBFC — inspired by Acer NitroSense.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Platform-Linux-A3E635?style=flat-square&labelColor=171717" alt="Linux">
  <img src="https://img.shields.io/badge/Shell-Bash-FF66C4?style=flat-square&labelColor=171717" alt="Bash">
  <img src="https://img.shields.io/badge/Backend-NBFC-A3E635?style=flat-square&labelColor=171717" alt="NBFC">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-FF66C4?style=flat-square&labelColor=171717" alt="MIT License"></a>
</p>

<p align="center">
  <a href="#-features">Features</a> ·
  <a href="#-preview">Preview</a> ·
  <a href="#-installation">Installation</a> ·
  <a href="#-commands">Commands</a> ·
  <a href="#-roadmap">Roadmap</a>
</p>

---

## ◈ Overview

**NeoSense** brings a clean, straightforward command-line experience to **NBFC (Notebook FanControl)** on Linux.

Control fan speeds, switch performance profiles, manage the NBFC service, and inspect your system's thermal status without memorizing a collection of low-level commands.

Built with modular Bash scripts, NeoSense aims to make laptop thermal management more accessible while keeping the interface lightweight, readable, and extensible.

> NeoSense is a wrapper around NBFC, not a replacement for it. Hardware compatibility and low-level fan control depend on your NBFC installation and configuration.

## ⌁ Preview

A quick look at the NeoSense command-line interface:

```text
  _   _            ____                         
 | \ | | ___  ___ / ___|  ___ _ __  ___  ___    
 |  \| |/ _ \/ _ \\___ \ / _ \ '_ \/ __|/ _ \   
 | |\  |  __/ (_) |___) |  __/ | | \__ \  __/   
 |_| \_|\___|\___/|____/ \___|_| |_|___/\___|   

  NeoSense v1.0
  A modern NBFC wrapper

  COMMANDS
  ├─ status          System & thermal status
  ├─ auto / manual   Fan control modes
  ├─ set             Manual fan speeds
  ├─ turbo            Maximum fan preset
  ├─ performance      Performance preset
  ├─ balanced         Everyday preset
  ├─ silent           Low-noise preset
  ├─ daemon           Service management
  └─ config           NBFC profiles
```

*Illustrative layout of the available commands, not a verbatim terminal capture.*

---

## ⚡ Features

<table>
  <tr>
    <td width="50%">
      <h3>◉ Fan control</h3>
      Switch between automatic and manual operation, or set CPU and GPU fan speeds individually or together.
    </td>
    <td width="50%">
      <h3>⚡ Performance presets</h3>
      Quickly select Turbo, Performance, Balanced, or Silent fan profiles.
    </td>
  </tr>
  <tr>
    <td width="50%">
      <h3>⌖ Thermal monitoring</h3>
      Inspect the active configuration, service state, temperatures, and current and target fan speeds.
    </td>
    <td width="50%">
      <h3>⚙ Service management</h3>
      Enable, disable, restart, and inspect the NBFC systemd service directly from the CLI.
    </td>
  </tr>
  <tr>
    <td width="50%">
      <h3>▦ Configuration profiles</h3>
      List available NBFC profiles, inspect the active profile, and apply a different configuration.
    </td>
    <td width="50%">
      <h3>⌘ Lightweight architecture</h3>
      Modular Bash scripts with separate responsibilities for commands, system operations, configuration, and output.
    </td>
  </tr>
</table>

---

## ▸ Installation

### Requirements

- A Linux distribution
- Bash and standard system utilities
- [NBFC Linux](https://github.com/nbfc-linux/nbfc-linux) installed and configured
- `sudo` privileges for operations that require elevated permissions

NBFC support varies by laptop model. Confirm that your hardware is supported and that its configuration works before using NeoSense to control the fans.

### 1. Install NBFC

On Arch Linux or Manjaro, if the package is available in your configured repositories or AUR helper:

```bash
yay -S nbfc-linux
```

On other distributions, follow the installation instructions in the [official nbfc-linux repository](https://github.com/nbfc-linux/nbfc-linux).

### 2. Install NeoSense

Clone the repository:

```bash
git clone https://github.com/NeonMC23/NeoSense.git
cd NeoSense
```

Run the installer:

```bash
sudo ./install.sh
```

The installation script is intended to:

- Check that NBFC is installed.
- Install NeoSense files under `/usr/local/share/neosense/`.
- Create the `neosense` command under `/usr/local/bin/`.

After installation, display the available commands:

```bash
neosense --help
```

Check the installed version:

```bash
neosense --version
```

---

## ⌘ Commands

### System status

```bash
neosense status
```

Displays the active NBFC configuration, service state, autostart state, operating mode, temperatures, and current and target fan speeds when the underlying system exposes those values.

### Fan control

| Command | Description |
|---|---|
| `neosense auto` | Switch all fans to automatic mode |
| `neosense manual` | Switch to manual mode while keeping current speeds |
| `neosense set cpu <0-100>` | Set the CPU fan speed |
| `neosense set gpu <0-100>` | Set the GPU fan speed |
| `neosense set both <0-100>` | Set both fan speeds |

Examples:

```bash
neosense auto
neosense manual
neosense set cpu 75
neosense set gpu 65
neosense set both 80
```

### Performance presets

Choose a preset according to your workload:

<table>
  <tr>
    <th>Preset</th>
    <th>CPU fan</th>
    <th>GPU fan</th>
    <th>Typical use</th>
  </tr>
  <tr>
    <td>🔥 <strong>Turbo</strong></td>
    <td>100%</td>
    <td>100%</td>
    <td>Maximum cooling</td>
  </tr>
  <tr>
    <td>⚡ <strong>Performance</strong></td>
    <td>80%</td>
    <td>80%</td>
    <td>Gaming and heavy workloads</td>
  </tr>
  <tr>
    <td>◈ <strong>Balanced</strong></td>
    <td>60%</td>
    <td>60%</td>
    <td>Everyday use</td>
  </tr>
  <tr>
    <td>❄️ <strong>Silent</strong></td>
    <td>25%</td>
    <td>25%</td>
    <td>Light workloads</td>
  </tr>
</table>

```bash
neosense turbo
neosense performance
neosense balanced
neosense silent
```

*Preset percentages are the values defined by NeoSense; actual fan behavior depends on hardware and NBFC support.*

### NBFC service

| Command | Action |
|---|---|
| `neosense daemon enable` | Enable and start the service |
| `neosense daemon disable` | Stop and disable the service |
| `neosense daemon restart` | Restart the service |
| `neosense daemon status` | Inspect the service state |

### Configuration profiles

| Command | Action |
|---|---|
| `neosense config current` | Show the active profile |
| `neosense config list` | List available NBFC profiles |
| `neosense config use "Acer Nitro AN515-57"` | Apply a profile and restart the service |

Use the exact configuration name reported by `neosense config list` on your system.

---

## ⚠️ Safety & compatibility

Fan control interacts with hardware-specific behavior. Incorrect settings can lead to excessive temperatures or unnecessary noise.

- Verify that NBFC supports your laptop and that its configuration is correct.
- Monitor CPU and GPU temperatures after changing fan settings.
- Avoid low manual fan speeds under sustained heavy workloads.
- Remember that a requested fan percentage does not guarantee a particular temperature.
- Use elevated privileges only when required by the operation.

NeoSense delegates hardware-level control to NBFC; it cannot guarantee compatibility with every laptop.

---

## ⛭ Project structure

```text
NeoSense/
├── README.md
├── LICENSE
├── install.sh
├── uninstall.sh
├── neosense
└── src/
    ├── commands.sh
    ├── system.sh
    ├── config.sh
    ├── status.sh
    └── utils.sh
```

| File | Responsibility |
|---|---|
| `neosense` | Main CLI entry point |
| `src/commands.sh` | Fan control commands and presets |
| `src/system.sh` | Temperatures and systemd service operations |
| `src/config.sh` | NBFC configuration profiles |
| `src/status.sh` | Status parsing and dashboard output |
| `src/utils.sh` | Colors, logging, and validation |
| `install.sh` | Installation |
| `uninstall.sh` | Uninstallation |

The modular structure keeps command handling separate from system operations and configuration management.

---

## ❓ Troubleshooting

<details>
  <summary><strong>“This command requires root privileges”</strong></summary>

  Some operations require elevated permissions to communicate with the underlying hardware or manage the system service. Run the relevant command with the required privileges and ensure that NBFC is configured correctly.
</details>

<details>
  <summary><strong>GPU temperature appears as N/A</strong></summary>

  Check whether your GPU driver exposes temperature information. On NVIDIA systems, verify that `nvidia-smi` is available. On AMD systems, check the driver's hardware monitoring interfaces. Not every GPU or driver exposes the same information.
</details>

<details>
  <summary><strong>Fan control does not work as expected</strong></summary>

  Confirm that NBFC is running, the selected profile matches your laptop, and the hardware is supported. Check NBFC's own status and logs before assuming the issue comes from NeoSense.
</details>

<details>
  <summary><strong>How do I uninstall NeoSense?</strong></summary>

  From the cloned repository directory, run:

  ```bash
  sudo ./uninstall.sh
  ```

  Review the script and its intended changes before running it.
</details>

---

## ◈ Roadmap

Potential future improvements:

- [ ] **Interactive TUI** — Live status and interactive fan controls.
- [ ] **GTK interface** — A graphical application with a system-tray indicator.
- [ ] **Custom fan curves** — Configure temperature-to-speed profiles.
- [ ] **Automatic gaming profile** — Switch profiles when games launch.
- [ ] **Smart battery mode** — Adapt profiles when unplugging the charger.
- [ ] **Desktop notifications** — Notify users about profile changes and temperature events.

These items are planned possibilities, not promises or implemented features.

---

## 🤝 Contributing

Contributions, bug reports, and feature suggestions are welcome.

1. Fork the repository.
2. Create a feature branch: `git checkout -b feature/my-feature`.
3. Commit your changes.
4. Push your branch.
5. Open a Pull Request.

For significant changes, open an issue first to discuss the proposed approach.

## 📄 License

NeoSense is distributed under the **MIT License**. See [`LICENSE`](LICENSE) for details.

<p align="center">
  <sub>Less command juggling. More control.</sub>
  <br>
  <strong><span style="color:#FF66C4">Neo</span><span style="color:#A3E635">Sense</span></strong>
</p>
