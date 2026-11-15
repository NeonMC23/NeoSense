//! Read-only Acer Embedded Controller tachometer candidates.
//!
//! The AN515 family exposes 16-bit fan tachometer candidates in EC bytes
//! 0x13/0x14 (channel 0) and 0x15/0x16 (channel 1). The byte order and
//! channel mapping are documented by the community `acer-nitro-ec` driver,
//! but have not been independently calibrated on this machine. Samples are
//! therefore marked provisional and are never eligible as feedback or control
//! evidence. This module only opens the EC debugfs file for reading.

use super::{ProviderOutput, SysfsRoot};
use crate::model::{
    SensorKind, SensorProvider, SensorRole, SensorSample, SensorStatus, Unit, now_ms,
};
use std::fs::File;
use std::io::Read;

const EC_IO_PATH: &str = "kernel/debug/ec/ec0/io";
const EC_SIZE: usize = 256;
// (channel, high-byte register, low-byte register)
const CHANNELS: [(u32, usize, usize); 2] = [(0, 0x13, 0x14), (1, 0x15, 0x16)];

pub fn collect(root: &SysfsRoot) -> ProviderOutput {
    let path = root.path.join(EC_IO_PATH);
    if !path.exists() {
        return ProviderOutput::default();
    }
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            return ProviderOutput::error(
                SensorProvider::Ec,
                format!("open {}: {error}", path.display()),
            );
        }
    };
    let mut data = [0u8; EC_SIZE];
    if let Err(error) = file.read_exact(&mut data) {
        return ProviderOutput::error(
            SensorProvider::Ec,
            format!("read {}: {error}", path.display()),
        );
    }

    let sampled_at = now_ms();
    let mut sensors = Vec::new();
    for (channel, high, low) in CHANNELS {
        let raw = ((data[low] as u16) << 8) | data[high] as u16;
        let mut sample = SensorSample::new(
            format!("acer_ec:fan{channel}.rpm"),
            SensorRole::FanRpm(channel),
            SensorKind::Rpm,
            Some(f64::from(raw)),
            Unit::Rpm,
            Some(format!("0x{:02x}:0x{:02x}", data[high], data[low])),
            "ec_u16_provisional",
            SensorProvider::Ec,
            "acer_ec".to_string(),
            Some(path.display().to_string()),
            Some(format!("EC tach {channel} (provisional)")),
            Some("acer-ec:ec0".to_string()),
        );
        sample.sampled_at_ms = sampled_at;
        if raw > 30_000 {
            sample = sample.invalid(format!("implausible EC tachometer estimate: {raw}"));
        } else {
            sample.status = SensorStatus::Ok;
        }
        sensors.push(sample);
    }
    ProviderOutput {
        sensors,
        errors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Trust;

    fn write_ec(dir: &tempfile::TempDir, bytes: &[u8]) {
        let path = dir.path().join(EC_IO_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn reads_provisional_dual_tachometers_without_enabling_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = vec![0u8; EC_SIZE];
        data[0x13] = 0x38;
        data[0x14] = 0x0f;
        data[0x15] = 0x8f;
        data[0x16] = 0x14;
        write_ec(&dir, &data);
        let output = collect(&SysfsRoot::new(dir.path()));
        assert!(output.errors.is_empty());
        assert_eq!(output.sensors.len(), 2);
        let cpu = &output.sensors[0];
        let gpu = &output.sensors[1];
        assert_eq!(cpu.value, Some(3896.0));
        assert_eq!(gpu.value, Some(5263.0));
        assert_eq!(cpu.health.trust, Trust::Provisional);
        assert!(!cpu.health.usable_for_feedback);
        assert!(!cpu.health.usable_for_regulation);
        assert!(cpu.health.physical_measurement);
    }

    #[test]
    fn missing_ec_interface_is_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let output = collect(&SysfsRoot::new(dir.path()));
        assert!(output.sensors.is_empty());
        assert!(output.errors.is_empty());
    }

    #[test]
    fn short_ec_read_is_an_error_not_a_zero() {
        let dir = tempfile::tempdir().unwrap();
        write_ec(&dir, &[0u8; 8]);
        let output = collect(&SysfsRoot::new(dir.path()));
        assert!(output.sensors.is_empty());
        assert_eq!(output.errors.len(), 1);
        assert!(output.errors[0].message.contains("read"));
    }
}
