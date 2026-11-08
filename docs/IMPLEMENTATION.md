# NeoSense — État de l’implémentation (v2.2.0)

**Date :** 2026-11-08
**Base :** audit technique précédent (`docs/audit/NEOSENSE_TECHNICAL_AUDIT.md`)
**Système/watchdog :** [`SYSTEMD.md`](SYSTEMD.md)
**TUI :** [`TUI.md`](TUI.md)
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
| Absence de tests/CI | **Résolu** | 43 tests unitaires + 15 tests CLI + 2 tests installateurs, CI fmt/clippy/test/shellcheck/systemd-analyze |
| Version incohérente | **Résolu** | `Cargo.toml` est la source unique ; `VERSION` partagée |
| Config fallback `SelectedConfigName` | **Obsolète** | Le backend lit le statut NBFC, pas `/etc/nbfc/nbfc.json` |
| GPU fan piloté par `@CPU` | **Signalé** | `diagnose` le détecte ; contrôle manuel GPU refusé sans source GPU explicite |
| TUI | **Implémentée (lecture seule par défaut)** | Ratatui/Crossterm, 5 vues, contrôle auto via IPC confirmé ; pas de vitesse manuelle |
| Accès IPC root-only | **Résolu** | Groupe `neosense`, socket `root:neosense` `0660`, runtime `0750` |
| Watchdog systemd | **Implémenté** | `Type=notify`, `WatchdogSec=10s`, READY/WATCHDOG/STOPPING via `sd-notify` |
| Récupération après SIGKILL | **Atténué** | `ExecStopPost=--fail-safe`, vérification auto ; non testé sur arrêt système réel |
| Profils en boucle fermée | **Différé** | Catalogue quiet/balanced/performance en aperçu ; seul `safe-auto` applicable |
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
cargo test --all-targets --all-features    OK (43 unitaires + 15 CLI + 2 installateurs)
shellcheck install.sh uninstall.sh         OK
bash -n install.sh uninstall.sh            OK
systemd-analyze verify (unité neutralisée) OK
```

Les tests n’effectuent aucune écriture ventilateur. Le superviseur est testé via un backend factice, un socket temporaire et un notifier factice. Les tests CLI utilisent le matériel réel uniquement en lecture. Les installateurs sont testés en dry-run dans des répertoires temporaires.

## 6.1 Groupe et socket IPC

- L’installateur crée le groupe système `neosense` s’il est absent, sans ajouter automatiquement d’utilisateur.
- Le socket est `root:neosense`, mode `0660`; le répertoire runtime est `root:neosense`, mode `0750`.
- Le superviseur applique explicitement le mode et le groupe avant `READY=1`, avec un `umask` restrictif pendant la création.
- Un utilisateur hors groupe reçoit le code 4 et un message indiquant d’ajouter le compte au groupe et de reconnecter la session.
- **Limite :** le socket NBFC `0666` reste un contournement local ; le groupe ne protège que NeoSense.

## 6.2 Watchdog et cycle de vie

- `Type=notify`, `WatchdogSec=10s`.
- READY après bind/permissions/premier échantillon ; WATCHDOG seulement si `Daemon::health()` est sain ; STOPPING à l’arrêt.
- Santé : fraîcheur de l’échantillon, cohérence de l’état, bail manuel, NBFC disponible pendant un contrôle manuel.
- Une indisponibilité NBFC en mode auto ne stoppe pas le heartbeat et ne déclenche pas d’écriture.
- `SIGKILL` ne peut pas être traité par Rust ; `ExecStopPost` exécute `--fail-safe`, qui appelle `nbfc set -a`, relit le statut et vérifie `AutoMode`.
- `--fail-safe` est idempotent et retourne une erreur si la vérification échoue.
- Test synthétique `sd_notify` (socket datagramme + `WATCHDOG_USEC`) : READY et WATCHDOG observés ; test non réalisé sous un systemd réel.
- Non testé : installation réelle de l’unité, vrai watchdog systemd, SIGKILL système réel.

## 7. Risques restants

- NBFC 0.5.3 ne permet pas le contrôle manuel protégé ; c’est volontaire.
- Le socket NBFC `0666` permet toujours un contournement local ; le groupe `neosense` ne protège que NeoSense.
- Le socket NeoSense est `root:neosense` `0660` ; le contrôle non-root exige l’appartenance au groupe et une nouvelle session.
- Le watchdog n’a pas été exercé par un systemd réel sur l’hôte.
- `ExecStopPost`/SIGKILL n’ont pas été testés sur un arrêt système réel.
- Les RPM sont souvent absents ; la vérification reste limitée à la cible.
- `nbfc rate-config` exige root ; `hardware validate` doit être exécuté avec les privilèges appropriés.
- Une seule machine a été vérifiée en lecture seule.
- Les courbes/profils en boucle fermée restent à implémenter et à valider.
- La TUI a été lancée dans un pty 100×32 (smoke test) ; une revue visuelle humaine des captures reste à faire.
- Le groupe réel `neosense` et l’unité système racine n’ont pas pu être testés sans mot de passe sudo ; l’intégration `systemd --user` transitoire a été testée avec un backend factice.
- Le contrôle manuel (dont hold) reste refusé sur NBFC 0.5.3.

## 8. Prochaine étape recommandée

1. Valider sur une machine dotée de NBFC ≥ 0.5.4 et d’un accès RPM :
   - `neosense diagnose --json` en lecture seule ;
   - `fan auto` via superviseur ;
   - un test manuel supervisé, court et borné.
2. Tester l’installation réelle de l’unité, le watchdog et `ExecStopPost` sur une VM systemd.
3. Ajouter la matrice de compatibilité et les profils en boucle fermée après validation matérielle.
4. Étendre la TUI à un contrôle manuel uniquement après validation physique complète.
