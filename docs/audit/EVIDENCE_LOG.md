# NeoSense Audit — Evidence Log

**Captured:** 2026-11-08 (Europe/Paris)  
**Purpose:** reproducible, read-only evidence supporting `NEOSENSE_TECHNICAL_AUDIT.md`.  
**Constraints:** no fan writes, no root, no package installation, no system changes. All commands below are read-only except where explicitly marked as a safe invalid-argument probe.

---

## 1. Environment

```text
$ grep PRETTY_NAME /etc/os-release
PRETTY_NAME="Nobara Linux 44 (KDE Plasma Desktop Edition)"

$ uname -r
7.3.0-203.nobara.fc44.x86_64

$ cat /sys/class/dmi/id/sys_vendor
Acer
$ cat /sys/class/dmi/id/product_name
Nitro AN515-57
$ cat /sys/class/dmi/id/product_version
V1.17
$ cat /sys/class/dmi/id/bios_version
V1.17

$ grep -m1 'model name' /proc/cpuinfo
model name : 11th Gen Intel(R) Core(TM) i5-11400H @ 2.70GHz

$ lspci | grep -Ei 'vga|3d|display'
0000:00:02.0 VGA compatible controller: Intel Corporation TigerLake-H GT1 [UHD Graphics]
0000:01:00.0 VGA compatible controller: NVIDIA Corporation GA107M [GeForce RTX 3050 Mobile]

$ nbfc --version
nbfc 0.5.3
```

NBFC upstream at audit time: 0.5.4, released 2026-10-10. The installed 0.5.3 predates the JSON status/reset-ec features used in the recommendations.

---

## 2. Git state at audit start

```text
$ git remote -v
origin  https://github.com/NeonMC23/NeoSense (fetch)
origin  https://github.com/NeonMC23/NeoSense (push)

$ git rev-parse HEAD
ad2cd8b71e5d91542fe74ac22b27bdb7ddb5cc23

$ git status --porcelain
(empty)

$ git tag -l
CLI-Releas-French
CLI-Release-NeoSense-v1.0.1

$ git ls-files -s
100644 ... 0  LICENSE
100644 ... 0  README.md
100644 ... 0  install.sh
100644 ... 0  neosense
100644 ... 0  src/commands.sh
100644 ... 0  src/config.sh
100644 ... 0  src/status.sh
100644 ... 0  src/system.sh
100644 ... 0  src/utils.sh
100644 ... 0  uninstall.sh
```

All scripts are mode `100644`, so `sudo ./install.sh` fails with "Permission denied" until the execute bit is added or the script is invoked with `bash`.

---

## 3. Static checks

```text
$ for f in neosense install.sh uninstall.sh src/*.sh; do bash -n "$f" && echo "OK $f"; done
OK neosense
OK install.sh
OK uninstall.sh
OK src/commands.sh
OK src/config.sh
OK src/status.sh
OK src/system.sh
OK src/utils.sh
```

ShellCheck 0.10.0 (`shellcheck -x -f gcc ...`) found no error-level diagnostics; only:

- SC1091 notes for dynamic `source` paths (not followed),
- SC2012 note for `ls` in `src/config.sh:61`,
- SC2034 warnings for unused parsed fan names and unused colour variables.

ShellCheck does not detect the semantic parsing/state bugs below.

---

## 4. Reproduced `status` failure

```text
$ NO_COLOR=1 bash neosense status
/home/neon/Documents/CODE LOCAL/NeoSense/src/status.sh: line 134: [: : integer expected
NeoSense v1.0

Configuration :
 Acer Nitro AN515-57 Custom

Service :
 Running

Autostart :
 Enabled

Mode :
 Auto

CPU
  Temp      58°C
  Current   0%
  Target    0%

GPU
  Temp      52°C
  Current   N/A
  Target    N/A
exit=0
```

The `integer expected` error comes from `[ "$fan_count" -ge 2 ]` with an empty `fan_count`. The dashboard still exits 0 and prints `0%` for unparsed fan speeds.

Actual NBFC output at the same time:

```text
$ nbfc status -a
Read-only                : false
Selected Config Name     : Acer Nitro AN515-57 Custom

Fan Display Name         : CPU fan
Temperature              : 49.88
Auto Control Enabled     : true
Critical Mode Enabled    : false
Current Fan Speed        : 100.00
Target Fan Speed         : 35.00
Fan Speed Steps          : 100

Fan Display Name         : GPU fan
Temperature              : 49.88
Auto Control Enabled     : true
Critical Mode Enabled    : false
Current Fan Speed        : 100.00
Target Fan Speed         : 40.00
Fan Speed Steps          : 100
```

The parser uses lowercase patterns (`Fan display name`, `Auto control enabled`, `Current fan speed`, `Target fan speed`, `Selected config name`), which do not match NBFC's Title-Case keys. `fan_count` is never incremented; AWK's uninitialised variable concatenates as an empty string, producing `FAN_COUNT:` with no value.

A fixture reproduction of the exact AWK block printed only `TEMP` lines and `FAN_COUNT:`, confirming the bug independently of the shell dashboard.

---

## 5. `manual` is invalid NBFC usage

Safe probe (invalid fan index, no speed; no hardware write):

```text
$ nbfc set -f 999
Usage: nbfc set [-h] (-a | -s PERCENT) [-f FAN INDEX]

Set the speed of fans.

If -f|--fan is not given, apply speed to all available fans.

Optional arguments:
  -h, --help            Show this help message and exit
  -a, --auto            Set fan speed to profile-based control
  -s PERCENT, --speed PERCENT
                        Set fan speed to PERCENT
  -f FAN INDEX, --fan FAN INDEX
                        Fan index (zero based)
exit=2
```

`src/commands.sh` calls `nbfc set -f 0` and `nbfc set -f 1` with neither `-s` nor `-a`; both necessarily exit 2. `manual` cannot work.

---

## 6. Mode-detection casing

NBFC prints lowercase booleans:

```text
Auto Control Enabled     : true
Critical Mode Enabled    : false
```

The exact comparison in `src/status.sh` is:

```bash
if [ "$fan_1_auto" = "False" ] || [ "$fan_2_auto" = "False" ]; then
```

A fixture simulation with `false` produced `Mode=Auto` in both the true and false cases, confirming that manual mode would never be displayed even if the fan parser were fixed.

---

## 7. GPU fallback selects a non-GPU sensor

`src/system.sh` GPU fallback:

```bash
sensors 2>/dev/null | grep -E '(edge|junction|mem|gpu|temp1)' | head -n 1 \
  | grep -oE '[+-][0-9.]+' | head -n 1 | cut -d'.' -f1 | tr -d '+-'
```

Observed `sensors` order and matching lines:

```text
coretemp-isa-0000
  Package id 0: +52.0°C
  Core 0:       +48.0°C
  ...
nvme-pci-e100
  Composite:    +43.9°C
...
iwlwifi_1-virtual-0
  temp1:        +54.0°C     <-- first generic "temp1" match
...
acpitz-acpi-0
  temp1:        +52.0°C
```

The first match is the **Intel Wi-Fi chip** (`iwlwifi_1`), not the NVIDIA GPU. If `nvidia-smi` is missing or returns multiple GPUs (newline-separated, rejected by `is_integer`), NeoSense reports Wi-Fi temperature as GPU temperature.

`nvidia-smi` itself returned a valid value at the same time:

```text
$ nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits
54
```

---

## 8. Config fallback key

```text
$ cat /etc/nbfc/nbfc.json
{
  "SelectedConfigId": "Acer Nitro AN515-57 Custom"
}

$ grep -i '"SelectedConfigName"' /etc/nbfc/nbfc.json | cut -d':' -f2 | tr -d '", \t'
(empty)
```

`src/config.sh` looks for `SelectedConfigName`, but current NBFC uses `SelectedConfigId`. The fallback therefore returns `"None"` when the service is stopped. `tr -d '", \t'` would also strip spaces from a matching name.

---

## 9. Root/socket security model

```text
$ ls -la /run/nbfc_service.socket
srw-rw-rw- 1 root root 0  7 nov. 13:47 /run/nbfc_service.socket

$ id -u
1000

$ nbfc set -f 999 -s 50
ERROR: nbfc: Service returned: Fan: No such fan available
```

The unprivileged request reached the service and was rejected only because fan index 999 does not exist. NBFC's upstream README states "Fan Control Rights: Any user"; the socket is chmod `0666` in `src/server.c`. A `require_root` wrapper does not create a security boundary.

---

## 10. Fan read-back is implausible on the audit machine

Config:

```text
Fan 0 (CPU fan): ReadRegister=19, WriteRegister=55,
                MinSpeedValue=0, MaxSpeedValue=100,
                IndependentReadMinMaxValues=false,
                MinSpeedValueRead=0, MaxSpeedValueRead=0,
                ResetRequired=true, FanSpeedResetValue=50
Fan 1 (GPU fan): same pattern
Model CriticalTemperature=95, EcPollInterval=1000, ReadWriteWords=true
```

NBFC journal:

```text
$ journalctl -u nbfc_service -n 6 --no-pager
nov. 08 17:20:54 ... CPU fan: Fan speed value (3488) not range of MinSpeedValueRead/MaxSpeedValueRead
nov. 08 17:20:54 ... GPU fan: Fan speed value (5084) not range of MinSpeedValueRead/MaxSpeedValueRead
nov. 08 17:20:55 ... CPU fan: Fan speed value (3488) not range of MinSpeedValueRead/MaxSpeedValueRead
nov. 08 17:20:55 ... GPU fan: Fan speed value (4687) not range of MinSpeedValueRead/MaxSpeedValueRead
```

NBFC clamps the raw EC value to its 0..100 read range; `nbfc status` therefore reports `Current Fan Speed: 100.00` while `Target Fan Speed` is 35/40%. `CurrentSpeed` is not RPM and is not trustworthy on this configuration.

---

## 11. Both fans use CPU sensors

```text
$ nbfc sensors show
#0 (CPU fan):
  Sensors:   @CPU (default)
  Algorithm: Average (default)

#1 (GPU fan):
  Sensors:   @CPU (default)
  Algorithm: Average (default)
```

NBFC's default is `@CPU` for every fan unless the model config specifies otherwise. On this machine, the GPU fan is driven by CPU temperature, not GPU temperature. NeoSense does not detect or disclose this.

---

## 12. NBFC state and service

```text
$ cat /var/lib/nbfc/state.json
{
  "TargetFanSpeeds": [
    -1.000000,
    -1.000000
  ]
}

$ systemctl show nbfc_service -p ActiveState,SubState,UnitFileState,MainPID,Restart
ActiveState=active
SubState=running
UnitFileState=enabled
MainPID=2755
Restart=on-failure
```

`-1` means auto. NBFC persists fixed target speeds in this file and re-applies them on restart; a low fixed speed can therefore survive a reboot. NBFC 0.5.4 adds `reset-ec` and an `ExecStopPost` in the systemd unit to improve crash recovery; 0.5.3's unit lacks it.

---

## 13. Status performance

```text
$ strace -f -c -e trace=execve bash neosense status
46 execve calls (15 errors)

$ /usr/bin/time -f '%e s' bash neosense status
0.05 s (warm)
1.59 s (cold)
```

A one-shot 50 ms CLI is acceptable; a 1 Hz TUI that shells out per frame is not. This supports a compiled core with in-process sampling.

---

## 14. Sensor inventory on the audit machine

```text
/sys/class/hwmon/hwmon0  name=ACAD
/sys/class/hwmon/hwmon1  name=acpitz         temp1=52000
/sys/class/hwmon/hwmon2  name=BAT1
/sys/class/hwmon/hwmon3  name=nvme           temp1=37850 temp2=37850 temp3=37850
/sys/class/hwmon/hwmon4  name=nvme           temp1=43850 temp2=43850 temp3=51850
/sys/class/hwmon/hwmon5  name=ucsi_source_psy_USBC000:001
/sys/class/hwmon/hwmon6  name=coretemp       temp1=63000 (Package id 0)
                                             temp2..7 = Core 0..5
/sys/class/hwmon/hwmon7  name=iwlwifi_1      temp1=51000
```

Thermal zones: `acpitz`, `INT3400 Thermal`, `SEN2`, `TCPU`, `SEN3`, `x86_pkg_temp`, `iwlwifi_1`.

No `fan*_input` is exposed by hwmon on this laptop; RPM is unavailable. NBFC reports only percentage values derived from EC registers, and the EC read-back is out of range for this config.

---

## 15. Upstream NBFC evidence

Inspected `nbfc-linux/nbfc-linux` commit `880087d` (2026-11-06):

- `src/client/cmd_status.c`: status options include `-j|--json`; human fields are Title Case.
- `src/client/cmd_set.c`: missing `-s`/`-a` prints help and returns `NBFC_EXIT_CMDLINE`.
- `src/client/cmd_config.c`: `Config_Apply()` prints "The -a|--apply option has been removed" and returns failure.
- `src/server.c`: status JSON keys `PID`, `SelectedConfigId`, `ReadOnly`, `Fans[]` (`Name`, `Temperature`, `CriticalTemperature`, `AutoMode`, `Critical`, `CurrentSpeed`, `TargetSpeed`, `RequestedSpeed`, `SpeedSteps`); socket chmod `0666`.
- `src/service.c`: persisted `TargetFanSpeeds` re-applied at startup; `ResetEC`; `Service_ShouldControlFans`; suspend/resume.
- `src/fan.c`: critical override returns 100%; fixed/auto modes; EC read range clamp.
- `src/main.c`: `atexit(Service_Cleanup)`; 100 consecutive failures then exit.
- `etc/systemd/system/nbfc_service.service.in`: `ExecStopPost=@BINDIR@/nbfc reset-ec`, `Restart=on-failure`.
- `CHANGELOG.md` 0.5.4: `status --json`, `reset-ec`, removed `config -a`, per-fan critical temperature, hardware-change detection.
- `PROTOCOL.md`: JSON + `\nEND` Unix socket protocol.
- `README.md`: "Fan Control Rights: Any user"; service control root-only.
- `src/ec_dummy.c`, `tools/test-protocol.py`: safe dummy EC/protocol test foundations.

Installed 0.5.3 does **not** recognize `nbfc status --json`:

```text
$ nbfc status --json
ERROR: nbfc: Unrecognized option: --json
```

This is why the backend must feature-detect, not assume.

---

## 16. Generated artifacts

The audit created only:

```text
docs/audit/NEOSENSE_TECHNICAL_AUDIT.md
docs/audit/EVIDENCE_LOG.md
docs/audit/README.md
```

No tracked source, manifest, lockfile, systemd unit, system configuration or installed file was changed. No commit/push was performed. All probes were read-only; the only fan-related command was an invalid-index `nbfc set` that the service rejected before any write.
