# NeoSense

**Surveillance thermique et gestion sûre des ventilateurs pour Linux.**

NeoSense est une interface moderne autour de **NBFC (Notebook FanControl)**. Le projet a été migré vers Rust pour garantir un modèle de capteurs typé, une politique de sécurité explicite et un superviseur unique des écritures ventilateur.

> NeoSense n’est pas un remplacement de NBFC. NBFC reste responsable de l’accès au contrôleur embarqué (EC), des configurations matérielles et de la protection critique du constructeur.

## État du projet

- **Version :** 2.0.0 (Rust).
- **Backend ventilateur :** NBFC.
- **Plateforme :** Linux uniquement.
- **TUI :** prévue, non implémentée dans cette version. La priorité actuelle est la correction, les capteurs et la sûreté.
- **Contrôle manuel :** disponible dans le code, mais **désactivé par la politique de sécurité** si NBFC < 0.5.4, si les capteurs sont absents/périmés, ou si le ventilateur GPU n’est pas piloté par une source GPU explicite.

## Fonctionnalités actuelles

| Commande | État |
|---|---|
| `neosense status` | Résumé CPU/GPU, service NBFC, configuration, ventilateurs |
| `neosense status --json` | Sortie JSON versionnée |
| `neosense sensors list` | Inventaire typé avec provenance, confiance, statut |
| `neosense sensors show <id>` | Détails bruts et normalisés d’un capteur |
| `neosense sensors watch` | Rafraîchissement en lecture seule |
| `neosense fan status` | Mode, cible, vitesse EC, source capteur |
| `neosense fan auto` | Retour au mode automatique NBFC via le superviseur |
| `neosense fan set` | Demande manuelle, soumise à la politique de sécurité |
| `neosense profile list` | Profils disponibles (`safe-auto` uniquement) |
| `neosense hardware list/current/validate` | Configurations NBFC et analyse `rate-config` |
| `neosense diagnose` | Diagnostic de compatibilité en lecture seule |
| `neosense service status` | État de `nbfc_service` et du superviseur |
| `neosense version` | Version |

Les commandes de lecture ne nécessitent pas le superviseur. Les commandes de contrôle passent **exclusivement** par `neosensed`.

## Installation

### Prérequis

- Linux avec NBFC installé et configuré.
- NBFC **0.5.4 ou plus récent recommandé** pour le contrôle manuel protégé (`status --json`, `reset-ec`, unité systemd avec `ExecStopPost`).
- NBFC 0.5.3 reste utilisable en lecture seule et pour `fan auto`/`diagnose`.
- Rust/Cargo pour une installation depuis les sources.
- `sudo` pour installer les binaires et l’unité systemd.

### Depuis les sources

```bash
git clone https://github.com/NeonMC23/NeoSense.git
cd NeoSense
sudo ./install.sh
```

L’installateur compile les binaires en mode release, installe :

- `/usr/local/bin/neosense`
- `/usr/local/bin/neosensed`
- `/etc/systemd/system/neosensed.service`

Il ne démarre pas le superviseur et ne modifie aucun réglage de ventilateur.

### Activer le superviseur

```bash
sudo systemctl enable --now neosensed
```

Le superviseur est l’autorité unique des écritures. Au démarrage, il remet NBFC en mode automatique si un état fixe est détecté. À l’arrêt, `ExecStopPost` exécute `neosensed --fail-safe` pour revenir au mode automatique et vérifier l’état.

### Désinstallation

```bash
sudo ./uninstall.sh
```

Le script arrête le superviseur (ce qui déclenche le retour automatique), supprime les binaires et l’unité. Il ne modifie pas la configuration NBFC.

## Utilisation

```bash
neosense status
neosense sensors list
neosense sensors show hwmon:coretemp:...:temp1
neosense fan status
neosense diagnose
neosense hardware list
neosense hardware current
sudo neosense hardware validate "Acer Nitro AN515-57 Custom"
```

`hardware validate` appelle `nbfc rate-config`, qui exige les privilèges root pour lire les tables ACPI.

Contrôle via le superviseur :

```bash
sudo systemctl start neosensed
sudo neosense fan auto
sudo neosense fan set cpu 50 --yes
```

Le socket IPC du superviseur est créé en `0660` ; dans cette version il appartient à `root:root`. Le contrôle nécessite donc encore `sudo` (ou une future appartenance de groupe dédiée). Les commandes de lecture ne nécessitent pas `sudo`.

`fan set` est refusé avec un code de sortie 8 si la politique de sécurité ne peut pas prouver que l’opération est sûre. Sur NBFC 0.5.3, le contrôle manuel est volontairement refusé.

### Sortie JSON et codes de sortie

Toutes les commandes de lecture acceptent `--json`. Les erreurs sont également sérialisées en JSON lorsque `--json` est actif.

| Code | Signification |
|---:|---|
| 0 | Succès |
| 2 | Argument invalide |
| 3 | Ressource introuvable |
| 5 | Diagnostic en échec |
| 7 | Backend/service/superviseur indisponible |
| 8 | Opération refusée pour raisons de sécurité |
| 10 | Écriture acceptée mais non vérifiée |
| 1 | Erreur générique |

## Sécurité

NeoSense applique les principes suivants :

1. **Le superviseur est le seul écrivain.** Le CLI et une future TUI ne touchent jamais directement au matériel.
2. **Pas de valeur inventée.** Un capteur absent, illisible, invalide ou périmé vaut `null`/`indisponible`, jamais `0 °C`.
3. **Pas de contrôle manuel sans preuves.** CPU/GPU valides, frais et suffisamment fiables ; plage de vitesse minimale ; plafond de température ; confirmation explicite ; bail temporaire.
4. **Pas de source GPU générique.** Un capteur Wi-Fi/NVMe/ACPI n’est jamais promu en GPU par un mot-clé. Le ventilateur GPU doit utiliser une source GPU explicite.
5. **Pas de promesse infondée.** Une vitesse EC n’est pas un RPM. Une commande acceptée n’est pas une vitesse vérifiée.
6. **Retour automatique.** À l’expiration du bail, en cas de capteur perdu, de service NBFC arrêté ou de température critique, le superviseur revient au mode auto.
7. **Pas de contournement des protections.** NeoSense ne désactive ni les limites kernel/firmware ni le mode critique NBFC.

### Limites connues

- NBFC 0.5.3 ne fournit pas `status --json` ni `reset-ec` : le contrôle manuel protégé est refusé. Utilisez NBFC ≥ 0.5.4.
- Le socket NBFC est accessible à tout utilisateur local (`0666`). NeoSense ne peut pas créer une frontière de sécurité que NBFC n’offre pas ; le socket NeoSense est en `0660` mais tout utilisateur peut appeler NBFC directement.
- De nombreux portables n’exposent pas de RPM via hwmon. La vérification est alors limitée à la cible NBFC.
- Certaines configurations NBFC pilotent le ventilateur GPU avec `@CPU`. `neosense diagnose` le signale ; le contrôle manuel GPU est refusé.
- Aucune TUI n’est encore disponible.
- Aucun profil à vitesse fixe (turbo/silent) n’est fourni : ce sont des politiques ouvertes dangereuses.

## Architecture

```text
neosense (CLI Rust)
   │  lecture directe (status/sensors/diagnose)
   │  ou socket Unix JSON
   ▼
neosensed (superviseur Rust)
   ├── capteurs : hwmon, thermal, power_supply, NVIDIA
   ├── backend NBFC : version, status JSON/texte, set/auto, rate-config
   ├── politique : fraîcheur, confiance, plafonds, TTL, sources GPU
   └── vérification : cible NBFC, mode, télémétrie EC
```

- `neosense status`, `sensors`, `diagnose` : lecture seule, sans superviseur.
- `neosense fan auto/set`, `profile apply` : IPC vers `neosensed`.
- `neosensed` : autorité unique ; aucune écriture si les preuves sont insuffisantes.
- Scripts shell : `install.sh`, `uninstall.sh`, unité systemd.

## Développement

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release --locked
```

Les tests n’effectuent **aucune écriture ventilateur**. Les tests unitaires couvrent les parseurs NBFC réels, les fixtures hwmon/thermal, les capteurs NVIDIA simulés, la politique de sécurité et l’IPC. Les tests d’intégration CLI n’utilisent pas le matériel.

L’état détaillé de la migration et les constats résolus/différés sont dans [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md).

### Tests matériels

Les tests matériels sont volontairement séparés. `neosense diagnose --json` est le point d’entrée en lecture seule. Aucune écriture, aucun reset EC et aucune modification firmware ne doit être effectuée sans validation supervisée.

## Compatibilité

| Élément | Statut |
|---|---|
| Acer Nitro AN515-57, i5-11400H, RTX 3050, NBFC 0.5.3 | Lecture seule vérifiée ; contrôle manuel refusé (version NBFC) |
| Autres portables | À valider individuellement ; ne pas supposer la compatibilité |
| NBFC 0.5.4+ | Recommandé pour le contrôle manuel protégé ; à tester sur le matériel cible |
| iGPU / GPU absent | Pas de température GPU inventée ; contrôle GPU indisponible |
| RPM absent | Vérification limitée ; signalé par `diagnose` |

NeoSense ne prétend pas contrôler tous les portables. La compatibilité dépend de NBFC, du modèle, du BIOS et de l’EC.

## Feuille de route

- Stabilisation des parseurs et de la politique (fait).
- Modèle de capteurs typé et diagnostic en lecture seule (fait).
- Superviseur et relais de contrôle sûrs (fondations faites).
- TUI Ratatui/Crossterm en lecture seule puis avec contrôle via IPC (à venir).
- Profils thermiques en boucle fermée et matrice de compatibilité (à venir).

## Licence

MIT. Voir [`LICENSE`](LICENSE).
