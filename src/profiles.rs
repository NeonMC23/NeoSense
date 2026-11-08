//! User-facing thermal profile catalogue.
//!
//! This release only enables `safe-auto`. Closed-loop profiles are represented
//! as previews so the UI and documentation can describe the intended model
//! without pretending that an unvalidated curve is safe to apply.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAvailability {
    Available,
    Preview,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAction {
    SafeAuto,
    ClosedLoop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalProfile {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub availability: ProfileAvailability,
    pub action: ProfileAction,
    pub reason: Option<&'static str>,
    pub safety: &'static str,
}

pub fn builtins() -> Vec<ThermalProfile> {
    vec![
        ThermalProfile {
            name: "safe-auto",
            title: "Automatique NBFC",
            description: "Confie le contrôle à la courbe automatique NBFC et vérifie l'état auto.",
            availability: ProfileAvailability::Available,
            action: ProfileAction::SafeAuto,
            reason: None,
            safety: "Aucune consigne de vitesse manuelle; ne garantit pas la vitesse physique.",
        },
        ThermalProfile {
            name: "quiet",
            title: "Silencieux",
            description: "Profil fermé à basse nuisance sonore, avec plancher et plafond thermiques.",
            availability: ProfileAvailability::Preview,
            action: ProfileAction::ClosedLoop,
            reason: Some(
                "Aperçu uniquement : courbe, hystérésis et mapping ventilateur non validés sur ce matériel.",
            ),
            safety: "Non applicable tant que les capteurs/fans et le readback ne sont pas validés.",
        },
        ThermalProfile {
            name: "balanced",
            title: "Équilibré",
            description: "Profil fermé équilibré entre acoustique et dissipation.",
            availability: ProfileAvailability::Preview,
            action: ProfileAction::ClosedLoop,
            reason: Some(
                "Aperçu uniquement : courbe non validée et contrôle manuel indisponible sur NBFC 0.5.3.",
            ),
            safety: "Non applicable; aucune écriture de vitesse fixe.",
        },
        ThermalProfile {
            name: "performance",
            title: "Performance",
            description: "Profil fermé privilégiant la dissipation thermique.",
            availability: ProfileAvailability::Preview,
            action: ProfileAction::ClosedLoop,
            reason: Some("Aperçu uniquement : validation thermique et readback physique requis."),
            safety: "Non applicable; ne remplace pas les protections NBFC/firmware.",
        },
        ThermalProfile {
            name: "custom",
            title: "Personnalisé",
            description: "Emplacement réservé à une future courbe utilisateur validée.",
            availability: ProfileAvailability::Preview,
            action: ProfileAction::ClosedLoop,
            reason: Some("Aucune configuration utilisateur validée dans cette version."),
            safety: "Non applicable.",
        },
    ]
}

pub fn find(name: &str) -> Option<ThermalProfile> {
    builtins().into_iter().find(|profile| profile.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_safe_auto_is_applicable() {
        let profiles = builtins();
        let applicable: Vec<&str> = profiles
            .iter()
            .filter(|p| p.availability == ProfileAvailability::Available)
            .map(|p| p.name)
            .collect();
        assert_eq!(applicable, vec!["safe-auto"]);
    }

    #[test]
    fn previews_have_a_reason() {
        for profile in builtins().into_iter().filter(|p| p.name != "safe-auto") {
            assert!(profile.reason.is_some(), "{} lacks a reason", profile.name);
            assert_ne!(profile.action, ProfileAction::SafeAuto);
        }
    }
}
