//! Fixed cooling presets exposed by the CLI and TUI.
//!
//! These are deliberately simple fixed-target presets, not hidden temperature
//! curves. Every command goes through the supervisor policy, backend
//! verification and the normal manual lease. The values are logical NBFC
//! targets, not measured physical RPM.

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
            description: "Fixed 25% target on both logical fan channels. Not a temperature curve.",
            cpu_percent: 25.0,
            gpu_percent: 25.0,
            safety: "Supervisor-gated fixed target; 15-minute manual lease, then automatic recovery.",
        },
        FanPreset {
            name: "balanced",
            title: "Balanced",
            description: "Fixed 60% target on both logical fan channels. Not a temperature curve.",
            cpu_percent: 60.0,
            gpu_percent: 60.0,
            safety: "Supervisor-gated fixed target; 15-minute manual lease, then automatic recovery.",
        },
        FanPreset {
            name: "maximum",
            title: "Maximum",
            description: "Fixed 100% target on both logical fan channels. Not a temperature curve.",
            cpu_percent: 100.0,
            gpu_percent: 100.0,
            safety: "Supervisor-gated fixed target; 15-minute manual lease, then automatic recovery.",
        },
    ]
}

fn legacy_performance() -> FanPreset {
    FanPreset {
        name: "performance",
        title: "Performance (legacy)",
        description: "Legacy fixed 80% target on both logical fan channels. Use Maximum for 100%.",
        cpu_percent: 80.0,
        gpu_percent: 80.0,
        safety: "Legacy alias; supervisor-gated fixed target with the same manual lease.",
    }
}

pub fn find(name: &str) -> Option<FanPreset> {
    let normalized = name.trim().to_ascii_lowercase();
    if let Some(preset) = builtins()
        .into_iter()
        .find(|preset| preset.name == normalized)
    {
        return Some(preset);
    }
    match normalized.as_str() {
        "max" | "turbo" => builtins()
            .into_iter()
            .find(|preset| preset.name == "maximum"),
        "performance" => Some(legacy_performance()),
        _ => None,
    }
}

pub fn names() -> Vec<&'static str> {
    builtins().into_iter().map(|preset| preset.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_presets_are_silent_balanced_and_maximum() {
        let presets = builtins();
        assert_eq!(presets.len(), 3);
        assert_eq!(
            presets
                .iter()
                .map(|preset| (preset.name, preset.cpu_percent, preset.gpu_percent))
                .collect::<Vec<_>>(),
            vec![
                ("silent", 25.0, 25.0),
                ("balanced", 60.0, 60.0),
                ("maximum", 100.0, 100.0),
            ]
        );
    }

    #[test]
    fn legacy_names_remain_resolvable() {
        assert_eq!(find("performance").unwrap().cpu_percent, 80.0);
        assert_eq!(find("turbo").unwrap().cpu_percent, 100.0);
        assert_eq!(find("max").unwrap().cpu_percent, 100.0);
        assert_eq!(find(" Maximum ").unwrap().name, "maximum");
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
