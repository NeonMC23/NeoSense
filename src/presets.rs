//! Historical fixed-speed presets, restored as explicit preset operations.
//!
//! These are not dynamic thermal curves. They request fixed per-fan targets and
//! still pass through the same supervisor policy and backend capability gates.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct FanPreset {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub cpu_percent: f64,
    pub gpu_percent: f64,
    pub safety: &'static str,
}

pub fn builtins() -> Vec<FanPreset> {
    vec![
        FanPreset {
            name: "silent",
            title: "Silent",
            description: "Historical fixed-speed preset: low target on each available fan.",
            cpu_percent: 25.0,
            gpu_percent: 25.0,
            safety: "Fixed targets are delivered only through the supervisor and policy gate.",
        },
        FanPreset {
            name: "balanced",
            title: "Balanced",
            description: "Historical fixed-speed preset: balanced acoustic target.",
            cpu_percent: 60.0,
            gpu_percent: 60.0,
            safety: "Fixed targets are delivered only through the supervisor and policy gate.",
        },
        FanPreset {
            name: "performance",
            title: "Performance",
            description: "Historical fixed-speed preset: higher cooling target.",
            cpu_percent: 80.0,
            gpu_percent: 80.0,
            safety: "Fixed targets are delivered only through the supervisor and policy gate.",
        },
        FanPreset {
            name: "turbo",
            title: "Turbo",
            description: "Historical fixed-speed preset: maximum permitted target.",
            cpu_percent: 100.0,
            gpu_percent: 100.0,
            safety: "Fixed targets are delivered only through the supervisor and policy gate.",
        },
    ]
}

pub fn find(name: &str) -> Option<FanPreset> {
    builtins().into_iter().find(|preset| preset.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_values_are_preserved() {
        assert_eq!(find("silent").unwrap().cpu_percent, 25.0);
        assert_eq!(find("balanced").unwrap().gpu_percent, 60.0);
        assert_eq!(find("performance").unwrap().cpu_percent, 80.0);
        assert_eq!(find("turbo").unwrap().gpu_percent, 100.0);
    }

    #[test]
    fn preset_names_are_unique() {
        let presets = builtins();
        for (index, preset) in presets.iter().enumerate() {
            assert!(
                !presets[..index]
                    .iter()
                    .any(|other| other.name == preset.name)
            );
        }
    }
}
