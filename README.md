# ❄️ NeoSense

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Linux-orange.svg)](#)
[![Dependency: NBFC](https://img.shields.io/badge/Dependency-NBFC-brightgreen.svg)](#)

**NeoSense** est un wrapper CLI (Command Line Interface) moderne, propre et élégant autour de **NBFC** (*Notebook FanControl*) sous Linux. Inspiré de l'interface de contrôle **NitroSense** d'Acer, NeoSense est conçu pour fonctionner sur n'importe quel ordinateur portable compatible avec NBFC.

Grâce à NeoSense, vous n'avez plus à mémoriser des commandes NBFC complexes. Contrôlez vos ventilateurs, changez de profil de performance en un clin d'œil, et surveillez l'état de votre matériel grâce à une interface colorée, lisible et hautement extensible.

---

## 🖥️ CAPTURE D'ÉCRAN (ASCII ART)

Voici à quoi ressemble la commande `neosense status` dans votre terminal :

```text
+-------------------------------------------------------------+
|  _   _               ____                                   |
| | \ | | ___  ___    / ___|  ___ _ __  ___  ___              |
| |  \| |/ _ \/ _ \   \___ \ / _ \ '_ \/ __|/ _ \             |
| | |\  |  __/ (_) |   ___) |  __/ | | \__ \  __/             |
| |_| \_|\___|\___/   |____/ \___|_| |_|___/\___|   v1.0      |
|                                                             |
+-------------------------------------------------------------+

Configuration :
  Acer Nitro AN515-57 Custom

Service :
  Running

Autostart :
  Enabled

Mode :
  Manual

CPU
  Temp      58°C
  Current   72%
  Target    70%

GPU
  Temp      53°C
  Current   64%
  Target    65%
```

---

## 🛠️ ARCHITECTURE DU PROJET

NeoSense a été pensé dès le premier jour pour être modulaire, propre et facilement extensible :

```text
NeoSense/
├── README.md           # Documentation complète
├── LICENSE             # Licence MIT
├── install.sh          # Script d'installation automatique
├── uninstall.sh        # Script de désinstallation automatique
├── neosense            # Script d'entrée CLI principal
└── src/                # Modules sources
    ├── commands.sh     # Commandes de contrôle des ventilateurs
    ├── system.sh       # Gestion des températures et du démon systemd
    ├── config.sh       # Gestion des profils de configuration de NBFC
    ├── status.sh       # Analyse de nbfc status et affichage du dashboard
    └── utils.sh        # Couleurs, journalisation et validations de base
```

---

## 🚀 INSTALLATION

### 1. Prérequis
NeoSense est un wrapper. Il nécessite donc que **NBFC (NoteBook FanControl)** soit déjà installé sur votre machine Linux.

* **Sur Arch Linux / Manjaro :**
  ```bash
  yay -S nbfc-linux
  ```
* **Sur Debian / Ubuntu / Fedora :**
  Référez-vous au dépôt officiel de [nbfc-linux](https://github.com/nbfc-linux/nbfc-linux) pour compiler ou installer le paquet correspondant.

### 2. Installer NeoSense
Clonez ce dépôt et exécutez le script d'installation :

```bash
git clone https://github.com/votre-username/NeoSense.git
cd NeoSense
sudo ./install.sh
```

Le script se chargera de :
1. Vérifier que `nbfc` est bien installé.
2. Déployer les fichiers de NeoSense dans `/usr/local/share/neosense/`.
3. Créer un lien symbolique exécutable dans `/usr/local/bin/neosense`.

---

## 📖 UTILISATION

Une fois installé, lancez simplement la commande `neosense` suivie de l'action de votre choix.

### Surveillance de l'état
```bash
neosense status
```
*Affiche la configuration active, l'état du service systemd, l'autostart, le mode actuel (Auto/Manual), les températures et vitesses de rotation réelles/cibles pour le CPU et le GPU.*

---

### Commandes de Contrôle des Ventilateurs

| Commande | Description | Équivalent NBFC |
| :--- | :--- | :--- |
| `neosense auto` | Passe tous les ventilateurs en mode automatique | `nbfc set -a` |
| `neosense manual` | Passe tous les ventilateurs en mode manuel sans changer les vitesses | `nbfc set -f 0` + `nbfc set -f 1` |
| `neosense set cpu <0-100>` | Définit la vitesse du ventilateur CPU de 0 à 100% | `nbfc set -f 0 -s XX` |
| `neosense set gpu <0-100>` | Définit la vitesse du ventilateur GPU de 0 à 100% | `nbfc set -f 1 -s XX` |
| `neosense set both <0-100>`| Définit la vitesse des ventilateurs CPU + GPU | `nbfc set -f 0/1 -s XX` |

---

### Profils Prédéfinis (Presets)

* **Turbo** (Vitesse maximale, refroidissement extrême) :
  ```bash
  neosense turbo
  ```
  *(CPU: 100% | GPU: 100%)*

* **Performance** (Jeux, tâches lourdes) :
  ```bash
  neosense performance
  ```
  *(CPU: 80% | GPU: 80%)*

* **Balanced** (Utilisation quotidienne) :
  ```bash
  neosense balanced
  ```
  *(CPU: 60% | GPU: 60%)*

* **Silent** (Bureautique, navigation web) :
  ```bash
  neosense silent
  ```
  *(CPU: 25% | GPU: 25%)*

---

### Gestion du Démon (nbfc_service)

* **Activer et démarrer au démarrage :**
  ```bash
  neosense daemon enable
  ```
* **Désactiver et arrêter :**
  ```bash
  neosense daemon disable
  ```
* **Redémarrer le démon :**
  ```bash
  neosense daemon restart
  ```
* **Afficher l'état du démon :**
  ```bash
  neosense daemon status
  ```

---

### Gestion des Configurations

* **Afficher le profil actif :**
  ```bash
  neosense config current
  ```
* **Lister les profils disponibles dans `/usr/share/nbfc/configs` :**
  ```bash
  neosense config list
  ```
* **Appliquer un profil et redémarrer automatiquement le service :**
  ```bash
  neosense config use "Acer Nitro AN515-57"
  ```

---

## ❓ FAQ

**Q : Pourquoi j'obtiens une erreur "This command requires root privileges" ?**  
**R :** Le contrôle des ventilateurs passe par l'écriture directe dans les registres du contrôleur embarqué (EC) de votre carte mère, ce qui nécessite des privilèges de super-utilisateur (`sudo`).

**Q : NeoSense indique "N/A" pour la température du GPU.**  
**R :** Si vous utilisez les pilotes propriétaires NVIDIA, assurez-vous que `nvidia-smi` est disponible. Pour les GPU AMD, vérifiez que le module `amdgpu` est chargé et exporte ses températures dans `sysfs`.

**Q : Comment désinstaller NeoSense ?**  
**R :** Exécutez simplement :  
`sudo ./uninstall.sh` depuis le dossier du projet.

---

## 🗺️ ROADMAP FUTURE

Grâce à sa structure modulaire en Bash, NeoSense est idéalement préparé pour intégrer de futures fonctionnalités majeures :

- [ ] **Interface TUI (Text User Interface) :** Création d'une interface de terminal interactive via `ratatui` ou `whiptail` pour surveiller les courbes en temps réel.
- [ ] **Interface Graphique GTK :** Application graphique avec indicateur dans la barre des tâches pour basculer facilement de profil.
- [ ] **Courbes de ventilation personnalisées :** Éditeur interactif de profils de température/vitesse.
- [ ] **Mode Gaming automatique :** Détection du lancement d'un jeu (Steam, Lutris) pour basculer automatiquement en profil Turbo.
- [ ] **Mode Batterie intelligent :** Passage automatique en profil Silent/Auto lors du débranchement du chargeur secteur.
- [ ] **Système de Notifications :** Alertes de bureau lors de changements de profils ou de pics de température de sécurité.

---

## 🤝 CONTRIBUTION

Les contributions sont les bienvenues ! 
1. Forkez le projet.
2. Créez une branche thématique (`git checkout -b feature/nouvelle-fonctionnalite`).
3. Commitez vos modifications.
4. Poussez votre branche (`git push origin feature/nouvelle-fonctionnalite`).
5. Ouvrez une **Pull Request**.

Pour toute modification majeure, veuillez d'abord ouvrir une issue pour discuter de ce que vous aimeriez changer.

---

## 📄 LICENCE

Ce projet est distribué sous **Licence MIT**. Voir le fichier [LICENSE](LICENSE) pour plus de détails.
