# Pont MPRIS pour Onde — Linux uniquement

Onde n'expose pas MPRIS : ce petit pont Python publie `org.mpris.MediaPlayer2.onde`
sur le bus de session pour que les shells et outils médias (Quickshell, Serpantinum,
`playerctl`, touches multimédias…) voient Onde comme un lecteur de musique normal.

Ce n'est **pas** une fonctionnalité d'Onde elle-même et cela ne fonctionne que sous
Linux (il lit `/proc/<pid>/fd` et dépend de D-Bus/Gio).

## Ce qui est exposé

- Métadonnées : titre, artiste, album, pochette (tags lus via `mutagen`,
  pochette extraite dans `~/.cache/onde-mpris/`).
- `PlaybackStatus` et `Position` estimés en suivant le descripteur de fichier
  qu'Onde lit réellement.
- Contrôles : lecture/pause, piste suivante/précédente. Onde n'ayant pas d'API
  distante, le pont injecte ses propres raccourcis clavier dans la fenêtre
  (`hyprctl eval` + `send_key_state`, sans voler le focus) — pensé pour Hyprland.
- `CanSeek` reste `false` : Onde ne permet pas de seek précis à distance.

## Installation

Dépendances : `python3`, `PyGObject` (`python-gobject` / `python3-gi` selon la
distribution) et `mutagen` (`python-mutagen` ou `pip install mutagen`).

```bash
install -Dm755 integrations/mpris/onde-mpris ~/.local/bin/onde-mpris
install -Dm644 integrations/mpris/onde-mpris.service ~/.config/systemd/user/onde-mpris.service
systemctl --user daemon-reload
systemctl --user enable --now onde-mpris.service
```

Onde apparaît alors dans la liste des lecteurs de votre shell (`playerctl -l`
le montre immédiatement).
