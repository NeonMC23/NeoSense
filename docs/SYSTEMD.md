# NeoSense — système, groupe et watchdog

**Public :** administrateurs et développeurs.
**État :** implémenté et testé en unités/CI ; non installé sur l’hôte d’audit.

## Groupe `neosense`

L’installation crée un groupe système `neosense` si nécessaire. Elle ne modifie pas l’appartenance des utilisateurs.

```bash
sudo usermod -aG neosense "$USER"
# Se déconnecter/reconnecter, ou :
newgrp neosense
```

Le socket `/run/neosense/neosensed.sock` est créé par le superviseur avec :

- propriétaire `root` ;
- groupe `neosense` ;
- mode `0660` ;
- répertoire `/run/neosense` en `root:neosense`, mode `0750`.

Le superviseur applique explicitement ces permissions au démarrage (`chmod`, `chown`) et échoue avant `READY=1` si le groupe demandé est introuvable ou si l’opération échoue. Le socket n’est jamais rendu accessible à `others`.

**Limite importante :** NBFC expose son propre socket en `0666`. Le groupe `neosense` ne protège que NeoSense. Tout utilisateur local peut encore piloter NBFC directement ; NeoSense ne prétend pas corriger cette limite upstream. Ne modifiez pas les permissions NBFC sans validation de l’éditeur et du matériel.

## Unité systemd

Le modèle est `packaging/neosensed.service`, installé vers `/etc/systemd/system/neosensed.service`.

Points clés :

- `Type=notify` et `NotifyAccess=main`.
- `WatchdogSec=10s`, `TimeoutStartSec=30s`, `TimeoutStopSec=10s`.
- `Restart=on-failure`, `RestartSec=2s`, limites de démarrage.
- `RuntimeDirectory=neosense`, mode `0750`, nettoyage à l’arrêt.
- `User=root`, `Group=neosense` : le superviseur peut appliquer l’appartenance du socket et communiquer avec NBFC ; le CLI n’a pas besoin de root s’il est membre du groupe.
- `NEOSENSE_SOCKET_GROUP=neosense` et `NEOSENSE_SOCKET=/run/neosense/neosensed.sock`.
- `ExecStopPost=/usr/local/bin/neosensed --fail-safe`.
- `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `ProtectKernelTunables`, `ProtectKernelModules`, `ProtectControlGroups`, `RestrictAddressFamilies=AF_UNIX AF_NETLINK`, `LockPersonality`, `SystemCallArchitectures=native`.

Les restrictions restent volontairement compatibles avec `nvidia-smi` et l’accès au socket NBFC. Aucun `PrivateDevices`, aucune liste de capacités minimaliste et aucun filtre `SystemCallFilter` n’a été ajouté sans validation matérielle ; ces restrictions pourraient casser l’accès GPU/NBFC sur certains systèmes.

L’installation ne démarre **pas** le service automatiquement.

## Watchdog

Le superviseur utilise `sd-notify` et envoie :

| État | Moment |
|---|---|
| `READY=1` | Après bind du socket, application des permissions et premier échantillon cohérent |
| `WATCHDOG=1` | Périodiquement, uniquement si `Daemon::health()` est sain |
| `STOPPING=1` | Pendant l’arrêt ordonné |

Critères de santé actuels :

- l’échantillon capteurs n’est pas plus vieux que `max(3 × interval, 5 s)` ;
- l’état interne n’est pas empoisonné ;
- un état `Manual` possède un bail cohérent et NBFC est disponible ;
- la boucle IPC principale répond (elle envoie les notifications).

Une indisponibilité NBFC en mode automatique met le service en état dégradé mais ne stoppe pas automatiquement le heartbeat : redémarrer le superviseur ne réparerait pas NBFC et pourrait provoquer une boucle de redémarrages. En mode manuel, NBFC indisponible rend le heartbeat unhealthy.

Le watchdog **ne garantit pas** :

- que le ventilateur physique tourne ;
- que l’EC répond correctement ;
- que les capteurs sont exacts ;
- qu’une protection firmware existe.

Il garantit seulement que le superviseur est vivant et cohérent selon ses propres critères. La vérification physique reste dépendante des RPM/capteurs disponibles.

## Cycle de vie et récupération

| Événement | Comportement |
|---|---|
| `SIGTERM` / `systemctl stop` | `STOPPING=1`, arrêt de la boucle, retour auto, suppression du socket, `ExecStopPost` |
| Erreur d’initialisation | Pas de `READY=1` ; systemd échoue le démarrage |
| `SIGKILL` / OOM | Aucun handler Rust ; systemd exécute `ExecStopPost` |
| Watchdog expiré | systemd tue/redémarre ; `ExecStopPost` s’exécute |
| Redémarrages répétés | `StartLimitBurst`/`RestartSec` bornent la boucle |
| NBFC déjà en mode fixe au démarrage | Le superviseur tente un retour auto vérifié (`fan auto`), sinon il refuse de démarrer |

`neosensed --fail-safe` :

1. appelle `nbfc set -a` ;
2. relit le statut NBFC ;
3. vérifie que tous les ventilateurs sont en mode auto ;
4. retourne un code non nul si la vérification échoue.

Il est idempotent et peut être répété. Il n’appelle pas `reset-ec`, ne modifie pas la configuration NBFC et ne suppose pas que `nbfc set -a` suffit à garantir une vitesse physique. Il dépend de la disponibilité du service NBFC et de la sémantique `set -a` de la version installée.

`nbfc set -a` est supporté par NBFC 0.5.3 (version auditée) et 0.5.4. Le helper ne revendique pas la compatibilité avec une version future : toute erreur de commande ou tout statut auto non confirmé produit un code de sortie non nul.

## Vérification locale

```bash
cargo test --all-targets --all-features
shellcheck install.sh uninstall.sh
```

La vérification `systemd-analyze` en CI remplace les chemins d’installation par un binaire neutre et le groupe par `root`, car l’unité n’est pas installée sur l’hôte de test. L’unité réelle n’a pas été installée ni démarrée pendant cette tâche.

## Limites restantes

- Le socket NBFC `0666` contourne le groupe `neosense`.
- Le contrôle manuel protégé reste refusé sur NBFC < 0.5.4.
- Aucun test réel de `ExecStopPost` sur arrêt système n’a été effectué.
- Le watchdog n’a pas été exercé par un vrai systemd sur l’hôte.
- Les opérations `hardware validate`/`rate-config` restent privilégiées.
