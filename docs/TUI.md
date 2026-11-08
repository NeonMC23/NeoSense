# NeoSense — TUI

La TUI est implémentée avec **Ratatui 0.30** et **Crossterm 0.29**. Elle partage le cœur Rust, le modèle de capteurs et le protocole IPC du CLI.

```bash
neosense tui
```

## Vues

| Vue | Contenu |
|---|---|
| Tableau de bord | CPU, GPU, ventilateurs, cible/EC, source capteur, mode, santé superviseur |
| Capteurs | Rôle, valeur, confiance, statut, source, chemin |
| Ventilateurs | Index, nom, rôle, mode, demandé, cible, EC, source |
| Profils | Catalogue et disponibilité ; seul `safe-auto` est applicable |
| Diagnostic | Version NBFC, service, config, capteurs, mapping GPU, RPM, socket, conflits |

## Interaction

| Touche | Action |
|---|---|
| `q`, Échap | Quitter |
| `Tab`, `→` / `Shift-Tab`, `←` | Vue suivante/précédente |
| `1`–`5` | Vue directe |
| `r` | Rafraîchir immédiatement |
| `a` | Retour auto NBFC, puis `y` confirmer / `n` annuler |
| `?` | Aide |

## Règles de sécurité

- La TUI **ne fait jamais** d’écriture directe vers NBFC.
- La seule action de contrôle est `fan.auto` via le socket superviseur, avec confirmation.
- Aucune commande de vitesse manuelle (0–100 %) n’est exposée dans la TUI.
- Sans superviseur accessible, la TUI est en lecture seule.
- Les valeurs EC sont affichées comme telles, jamais comme RPM.
- Une valeur absente est affichée `indisponible`/`?`, jamais `0`.

## Tests

- Rendu `TestBackend` 100×32 pour chaque vue.
- Navigation d’onglets et état de confirmation.
- Superposition d’aide.
- Aucun test TUI n’ouvre de socket superviseur ni n’écrit vers NBFC.

La TUI a été lancée dans un pty réel 100×32 (smoke test) et les cinq vues ont été parcourues ; les tests de rendu couvrent aussi 40×20, 80×24, 100×32 et 160×48. Une revue visuelle humaine des captures reste à faire.
