# NeoSense — État de l’implémentation (v2.0.0)

**Date :** 2026-11-08
**Base :** audit technique précédent (`docs/audit/NEOSENSE_TECHNICAL_AUDIT.md`)
**Objectif :** stabiliser l’existant, migrer l’application vers Rust et poser les fondations sûres du superviseur.

## 1. Décision d’architecture

- `neosense` : CLI Rust, lecture directe pour `status`/`sensors`/`diagnose`.
- `neosensed` : superviseur Rust, autorité unique des écritures ventilateur.
- `neosense-core` (module `src/`) : capteurs typés, backend NBFC, politique, IPC.
- Scripts shell : `install.sh`, `uninstall.sh`, unité systemd uniquement.
- NBFC reste le backend EC/ventilateur initial ; aucun code NBFC n’est vendu ou copié.

L’ancienne CLI Bash (`neosense`, `src/*.sh`) a été retirée pour éviter deux implémentations concurrentes. Les scripts d’installation restent en Bash.

## 2. Constats de l’audit : résolus, atténués ou différés

| Constat | État | Détail |
|---|---|---|
| Parseur `status` sensible à la casse | **Résolu** | Parseur NBFC texte exact + JSON 0.5.4, fixtures réelles, champs manquants = `None` |
| `manual` construisait `nbfc set -f N` invalide | **Résolu** | Le contrôle manuel passe par le superviseur et n’est autorisé que si la politique l’accepte |
| `false` comparé à `"False"` | **Résolu** | Booléens insensibles à la casse, test dédié |
| Repli GPU générique (`temp1`) | **Résolu** | Rôles GPU uniquement pour `amdgpu`/`nouveau`/`radeon`/NVIDIA identifiés ; Wi-Fi/NVMe/ACPI restent leurs propres rôles |
| `config use` avec `nbfc config -a` | **Atténué** | Plus d’application automatique ; `hardware validate` utilise `rate-config` (root requis), `hardware apply` n’est pas exposé |
| Vitesse EC présentée comme fiable | **Atténué** | `EC`, `cible`, `demandé` et RPM sont distincts ; marqueur `(!)` et diagnostic ; pas de RPM inventé |
| Presets fixes dangereux | **Retiré** | Aucun preset 0/25 % ; `safe-auto` est le seul profil |
| `require_root` comme frontière de sécurité | **Corrigé dans le discours** | Socket superviseur `0660` ; limite NBFC `0666` documentée ; aucune prétention de sécurité root |
| Scripts non exécutables | **Résolu** | Bits exécutables + installateur absolu + build Rust |
| Absence de tests/CI | **Résolu** | 29 tests unitaires + 7 tests CLI, CI fmt/clippy/test/shellcheck |
| Version incohérente | **Résolu** | `Cargo.toml` est la source unique ; `VERSION` partagée |
| Config fallback `SelectedConfigName` | **Obsolète** | Le backend lit le statut NBFC, pas `/etc/nbfc/nbfc.json` |
| GPU fan piloté par `@CPU` | **Signalé** | `diagnose` le détecte ; contrôle manuel GPU refusé sans source GPU explicite |
| TUI | **Différé** | Non implémentée ; priorité à la sûreté et aux fondations |
| Profils en boucle fermée | **Différé** | Nécessite validation matérielle et modèle de courbe |
| Compatibilité large | **Différée** | Une seule machine vérifiée en lecture seule ; matrice à alimenter |

## 3. Modèle de capteurs

Chaque `SensorSample` porte :

- identifiant stable (`hwmon:<chip>:<device>:tempN`, `thermal:...`, `nvidia:...`) ;
- rôle typé (`CpuPackage`, `CpuCore`, `GpuDie`, `GpuHotspot`, `GpuMemory`, `FanRpm`, `Battery`, `Nvme`, `Wifi`, `ThermalZone`, `Power`, `Utilization`, `Unknown`) ;
- valeur normalisée et unité ;
- valeur brute et unité brute ;
- fournisseur, chemin, label, identifiant d’appareil ;
- horodatage, statut, confiance, provenance ;
- erreur explicite.

États distincts : détecté, lisible, valide, frais, fiable, utilisable pour le contrôle. Une lecture absente ou invalide vaut `None`, jamais `0`.

### Sources

- hwmon : `coretemp`, `k10temp`, `zenpower`, `amdgpu`, `nouveau`, `radeon`, `nvme`, `iwlwifi`, `acpitz`, canaux `fan*_input`, `power*_input`.
- thermal : `thermal_zone*` uniquement ; `x86_pkg_temp` comme repli CPU.
- power_supply : température batterie en 1/10 °C.
- NVIDIA : `nvidia-smi` avec timeout, parsing CSV, multi-GPU, `N/A` = indisponible.

Aucune promotion d’un capteur générique `temp1` en GPU/CPU.

## 4. Politique de sécurité

Le contrôle manuel est refusé si :

- NBFC < 0.5.4 ;
- service en lecture seule ou mode critique ;
- capteur CPU/GPU absent, invalide, périmé ou insuffisamment fiable ;
- source capteur NBFC inconnue ;
- ventilateur GPU piloté par `@CPU`/autre source non GPU ;
- vitesse hors plage minimale ou > 100 % ;
- température au-dessus du plafond ;
- confirmation absente.

Le bail manuel expire (15 min par défaut). En cas d’expiration, de perte de capteur critique, de mode critique NBFC, d’échec de vérification ou de service perdu, le superviseur demande le retour auto et marque l’état `FailSafe` si la vérification échoue.

## 5. Vérification

| Niveau | Signification |
|---|---|
| accepté | NBFC a accepté la requête |
| cible confirmée | `TargetSpeed` correspond, `AutoMode=false` |
| télémétrie EC plausible | valeur dans 0–100 et proche de la cible |
| RPM confirmé | tachymètre hwmon disponible (souvent absent) |
| non vérifié | aucune preuve suffisante |

La télémétrie EC n’est jamais présentée comme un RPM. Sur la machine d’audit, `100 %` contre une cible de 35 % est marqué `(!)` et le diagnostic explique que la lecture n’est pas concluante.

## 6. Tests exécutés

```text
cargo fmt --all -- --check                 OK
cargo clippy --all-targets --all-features -- -D warnings  OK
cargo test --all-targets --all-features    OK (29 unitaires + 7 CLI)
shellcheck install.sh uninstall.sh         OK
bash -n install.sh uninstall.sh            OK
```

Les tests n’effectuent aucune écriture ventilateur. Le superviseur est testé via un backend factice et un socket temporaire. Les tests CLI utilisent le matériel réel uniquement en lecture.

## 7. Risques restants

- NBFC 0.5.3 ne permet pas le contrôle manuel protégé ; c’est volontaire.
- Le socket NBFC `0666` permet à tout utilisateur local de contourner NeoSense.
- Le socket NeoSense `0660` est actuellement root:root ; le contrôle depuis un utilisateur non root nécessite une évolution de groupe/polkit.
- Les RPM sont souvent absents ; la vérification reste limitée à la cible.
- `nbfc rate-config` exige root ; `hardware validate` doit être exécuté avec les privilèges appropriés.
- Une seule machine a été vérifiée en lecture seule.
- La TUI, les courbes et les profils matériels restent à implémenter.

## 8. Prochaine étape recommandée

1. Valider sur une machine dotée de NBFC ≥ 0.5.4 et d’un accès RPM :
   - `neosense diagnose --json` en lecture seule ;
   - `fan auto` via superviseur ;
   - un test manuel supervisé, court et borné.
2. Ajouter un groupe système `neosense` et un socket IPC `0660` partagé pour permettre un CLI/TUI non root.
3. Implémenter la TUI Ratatui/Crossterm en lecture seule, puis le contrôle via IPC uniquement.
4. Ajouter la matrice de compatibilité et les profils en boucle fermée après validation matérielle.
