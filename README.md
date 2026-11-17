# Onde

Lecteur de musique natif, rapide et léger, écrit en Rust avec [GPUI](https://gpui.rs).

![Linux](https://img.shields.io/badge/Linux-FCC624?logo=linux&logoColor=black)
![Windows](https://img.shields.io/badge/Windows-0078D6?logo=windows&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white)
![License](https://img.shields.io/github/license/dontleteuwatchyou/onde)
![Release](https://img.shields.io/github/v/release/dontleteuwatchyou/onde)

---

## Fonctionnalités

- Lecture de bibliothèque locale (FLAC, MP3, AAC, OGG, WAV)
- Téléchargement via **Telegram** (@DeezLoad2Bot, FLAC) ou **YouTube** (yt-dlp)
- Recherche dans le catalogue **Deezer**
- Thème dynamique **Pochette** — les couleurs suivent la pochette de l'album
- **Discord Rich Presence** avec pochettes hébergées
- Playlists, morceaux aimés, suggestions par artiste
- Visuels audio (spectrogramme, oscilloscope)
- Raccourcis clavier complets
- Mises à jour automatiques signées

## Installation

### Linux

Télécharge le binaire depuis la [dernière release](https://github.com/dontleteuwatchyou/onde/releases/latest) :

```bash
curl -Lo onde https://github.com/dontleteuwatchyou/onde/releases/latest/download/onde-linux-x86_64
chmod +x onde
mv onde ~/.local/bin/
```

### Windows

Télécharge `onde-windows-x86_64.exe` depuis la [dernière release](https://github.com/dontleteuwatchyou/onde/releases/latest).

### Depuis les sources

```bash
cargo install --path .
```

## Telegram (optionnel)

Onde peut télécharger de la musique en FLAC via le bot Telegram `@DeezLoad2Bot`.

1. Crée une application sur [my.telegram.org/apps](https://my.telegram.org/apps)
2. Configure les clés :
```bash
mkdir -p ~/.config/onde/telegram
echo 'ONDE_TELEGRAM_API_ID=ton_id' > ~/.config/onde/telegram/api.env
echo 'ONDE_TELEGRAM_API_HASH=ton_hash' >> ~/.config/onde/telegram/api.env
```
3. Installe le venv Python :
```bash
python3 -m venv ~/.local/share/onde/telegram-venv
~/.local/share/onde/telegram-venv/bin/pip install telethon
```
4. Connecte-toi via le QR code dans Paramètres → Telegram

## Compilation

Dépendances Linux :
```bash
sudo apt install libasound2-dev libvulkan-dev libwayland-dev libxkbcommon-dev libfontconfig1-dev
```

```bash
cargo build --release
```
