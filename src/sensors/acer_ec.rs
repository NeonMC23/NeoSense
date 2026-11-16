//! Read-only Acer Embedded Controller tachometer candidates.
//!
//! The AN515 family exposes 16-bit fan tachometer candidates in EC bytes
//! 0x13/0x14 (channel 0) and 0x15/0x16 (channel 1). The community
//! `acer-nitro-ec` driver assembles `(low << 8) | high` and performs no unit
//! conversion. Its mapping has not been independently calibrated on this
//! machine, so samples remain provisional and are never eligible as feedback
//! or control evidence.
//!
//! The kernel `ec_sys` read handler issues one `ec_read` per byte, so a full
//! 256-byte read is not atomic: a tachometer word can be captured while the
//! firmware updates it. This is another reason the assembled value is not
//! presented as validated RPM. Zero is passed through as a readable value
//! rather than replaced; `0xffff` and values above the sibling-model ceiling
//! are marked invalid. This module only opens the EC debugfs file for reading.

use super::{ProviderOutput, SysfsRoot};
use crate::model::{
    SensorKind, SensorProvider, SensorRole, SensorSample, SensorStatus, Unit, now_ms,
};
use std::fs::File;
use std::io::Read;

const EC_IO_PATH: &str = "kernel/debug/ec/ec0/io";
const EC_SIZE: usize = 256;
// (channel, high-byte register, low-byte register, mode register)
const CHANNELS: [(u32, usize, usize, usize); 2] = [(0, 0x13, 0x14, 0x22), (1, 0x15, 0x16, 0x21)];

/// NBFC read registers sampled by the provisional EC channels.
///
/// This is a register-identity mapping, not a physical-fan mapping. It lets
/// diagnostics compare NBFC's configured read register with the same raw EC
/// word NeoSense already exposes. It does not establish which physical fan is
/// attached to either channel.
pub const PROVISIONAL_READ_REGISTERS: [(u16, u32); 2] = [(0x13, 0), (0x15, 1)];

pub fn provisional_channel_for_read_register(register: u16) -> Option<u32> {
    PROVISIONAL_READ_REGISTERS
        .iter()
        .find_map(|(candidate, channel)| (*candidate == register).then_some(*channel))
}

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
    for (channel, high, low, mode_register) in CHANNELS {
        let raw = ((data[low] as u16) << 8) | data[high] as u16;
        let mut sample = SensorSample::new(
            format!("acer_ec:fan{channel}.rpm"),
            SensorRole::FanRpm(channel),
            SensorKind::Rpm,
            Some(f64::from(raw)),
            Unit::Raw,
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

        let mode = data[mode_register];
        let mut mode_sample = SensorSample::new(
            format!("acer_ec:fan{channel}.mode"),
            SensorRole::FanMode(channel),
            SensorKind::Mode,
            Some(f64::from(mode)),
            Unit::Code,
            Some(format!("0x{mode:02x}")),
            "ec_mode_code_provisional",
            SensorProvider::Ec,
            "acer_ec".to_string(),
            Some(path.display().to_string()),
            Some(format!("EC fan mode {channel} (provisional)")),
            Some("acer-ec:ec0".to_string()),
        );
        mode_sample.sampled_at_ms = sampled_at;
        mode_sample.status = SensorStatus::Ok;
        sensors.push(mode_sample);
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
        data[0x22] = 0x0c;
        data[0x21] = 0x30;
        write_ec(&dir, &data);
        let output = collect(&SysfsRoot::new(dir.path()));
        assert!(output.errors.is_empty());
        assert_eq!(output.sensors.len(), 4);
        let cpu = &output.sensors[0];
        let gpu = &output.sensors[2];
        let cpu_mode = &output.sensors[1];
        let gpu_mode = &output.sensors[3];
        assert_eq!(cpu.value, Some(3896.0));
        assert_eq!(gpu.value, Some(5263.0));
        assert_eq!(cpu.unit, Unit::Raw);
        assert_eq!(gpu.unit, Unit::Raw);
        assert_eq!(cpu.health.trust, Trust::Provisional);
        assert!(!cpu.health.usable_for_feedback);
        assert!(!cpu.health.usable_for_regulation);
        assert!(cpu.health.physical_measurement);
        assert_eq!(cpu_mode.value, Some(0x0c as f64));
        assert_eq!(cpu_mode.kind, SensorKind::Mode);
        assert_eq!(cpu_mode.unit, Unit::Code);
        assert_eq!(cpu_mode.health.trust, Trust::Provisional);
        assert!(!cpu_mode.health.usable_for_feedback);
        assert!(!cpu_mode.health.usable_for_regulation);
        assert_eq!(cpu_mode.raw_value, Some("0x0c".to_string()));
        assert_eq!(gpu_mode.value, Some(0x30 as f64));
        assert_eq!(gpu_mode.raw_value, Some("0x30".to_string()));
    }

    #[test]
    fn nbfc_read_registers_map_only_to_the_same_provisional_channels() {
        assert_eq!(provisional_channel_for_read_register(0x13), Some(0));
        assert_eq!(provisional_channel_for_read_register(0x15), Some(1));
        assert_eq!(provisional_channel_for_read_register(0x37), None);
        assert_eq!(provisional_channel_for_read_register(0x3a), None);
    }

    #[test]
    fn raw_word_is_low_byte_first_matching_upstream_driver() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = vec![0u8; EC_SIZE];
        // Upstream: speed = (ec[low] << 8) | ec[high], yielded as 0x0f38.
        data[0x13] = 0x38;
        data[0x14] = 0x0f;
        write_ec(&dir, &data);
        let output = collect(&SysfsRoot::new(dir.path()));
        let cpu = &output.sensors[0];
        assert_eq!(cpu.value, Some(0x0f38 as f64));
        assert_eq!(cpu.raw_value, Some("0x38:0x0f".to_string()));
    }

    #[test]
    fn sentinel_zero_is_not_substituted_and_implausible_values_are_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = vec![0u8; EC_SIZE];
        data[0x13] = 0xff;
        data[0x14] = 0xff;
        write_ec(&dir, &data);
        let output = collect(&SysfsRoot::new(dir.path()));
        assert_eq!(output.sensors[0].status, SensorStatus::Invalid);
        assert!(output.sensors[0].value.is_none());
        // A genuinely zero word remains a readable zero, not an invented value.
        let dir = tempfile::tempdir().unwrap();
        write_ec(&dir, &vec![0u8; EC_SIZE]);
        let output = collect(&SysfsRoot::new(dir.path()));
        assert_eq!(output.sensors[0].value, Some(0.0));
        assert_eq!(output.sensors[0].status, SensorStatus::Ok);
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
