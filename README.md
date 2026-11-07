<div align="center">

# 🌊 Onde

**Toute ta musique au même endroit, dans une vraie appli native.**
Un lecteur multi-source en Rust et GPUI, sans Electron : ta bibliothèque, ton serveur Navidrome, tes téléchargements, des visuels qui réagissent au son.

![Linux](https://img.shields.io/badge/Linux-pris%20en%20charge-2f55a8?logo=linux&logoColor=white)
![Windows](https://img.shields.io/badge/Windows-non%20pris%20en%20charge-555555?logo=windows&logoColor=white)
![macOS](https://img.shields.io/badge/macOS-non%20pris%20en%20charge-555555?logo=apple&logoColor=white)
![Wayland](https://img.shields.io/badge/test%C3%A9%20sous-Wayland%20%2F%20Hyprland-58a6ff)
![Rust](https://img.shields.io/badge/Rust-GPUI-dea584?logo=rust&logoColor=white)
[![Ko-fi](https://img.shields.io/badge/Ko--fi-soutenir-ff5e5b?logo=kofi&logoColor=white)](https://ko-fi.com/forsakengh)

<img src="docs/accueil.png" alt="L'accueil d'Onde pendant la lecture d'un morceau" width="90%">

</div>

---

## Ce qu'Onde sait faire

| | |
|---|---|
| 🎵 **Lecture** | Fichiers locaux, format réel affiché (FLAC 24 bits, 96 kHz…), lecture aléatoire |
| 📚 **Bibliothèque** | Morceaux, albums, artistes, tri et recherche |
| ❤️ **Collections** | Titres likés et playlists, qui survivent à un nouveau scan |
| 🖧 **Navidrome** | Ton serveur Subsonic ou Navidrome comme source |
| ⬇️ **Téléchargements** | YouTube et SoundCloud avec `yt-dlp`, et Live Music Archive |
| 🎤 **Paroles** | Paroles synchronisées quand elles existent |
| 🌊 **Visuels** | Cinq visuels qui réagissent à la musique : Marée, Ricochets, Banc, Méduse, Éclats |
| 🎨 **Thèmes** | Sept thèmes sombres et un thème clair |
| 🎮 **Discord** | Le morceau en cours sur ton profil, avec sa pochette et son temps |

<div align="center">
<img src="docs/plein-ecran.png" alt="Onde en plein écran avec un visuel" width="90%">
</div>

## Installation

> Le dépôt est privé : il faut un compte qui y a accès et [GitHub CLI](https://cli.github.com) connecté (`gh auth login`).

### En une commande

```bash
gh api -H 'Accept: application/vnd.github.raw' repos/dontleteuwatchyou/onde/contents/install.sh | sh
```

Le script télécharge la dernière version, vérifie l'archive et installe Onde pour ton utilisateur, sans `sudo`. Onde apparaît aussi dans ton lanceur d'applications. Pour le retirer, lance `sh install.sh --uninstall`.

### Avec la version compilée, étape par étape

Compilée sous Arch Linux pour x86_64. Sur une distribution avec une glibc plus ancienne, compile depuis les sources.

**1.** Installe ce qu'il faut pour le son et l'affichage. Sous Arch :

```bash
sudo pacman -S --needed alsa-lib libxcb libxkbcommon libxkbcommon-x11 vulkan-icd-loader wayland
```

Il te faut aussi le pilote Vulkan de ta carte graphique (`vulkan-radeon`, `vulkan-intel` ou `nvidia-utils`).

**2.** Télécharge et décompresse Onde :

```bash
gh release download -R dontleteuwatchyou/onde --pattern 'onde-*-linux-x86_64.tar.gz*'
```

```bash
sha256sum -c onde-*-linux-x86_64.tar.gz.sha256 && tar xzf onde-*-linux-x86_64.tar.gz
```

**3.** Installe-le et lance-le :

```bash
install -Dm755 onde-*-linux-x86_64/onde ~/.local/bin/onde
```

```bash
onde
```

Si la commande est introuvable, ajoute `~/.local/bin` à ton `PATH`.

### Depuis les sources

```bash
sudo pacman -S --needed base-devel rustup cmake pkgconf fontconfig freetype2 alsa-lib libxcb libxkbcommon libxkbcommon-x11 vulkan-icd-loader wayland
```

```bash
rustup default stable
```

```bash
gh repo clone dontleteuwatchyou/onde && cd onde && cargo build --release
```

```bash
./target/release/onde
```

### En option

Pour télécharger depuis YouTube et SoundCloud, installe `yt-dlp` et `ffmpeg` :

```bash
sudo pacman -S --needed yt-dlp ffmpeg
```

## Premiers pas

1. Au premier lancement, Onde te demande ton dossier de musique.
2. Pour ajouter ton serveur, va dans **Sources**, ou appuie sur `N`.
3. Pour changer de visuel ou de thème, va dans **Réglages**.

## Raccourcis

| Touche | Action |
|---|---|
| `Espace` | Lecture ou pause |
| `Ctrl` + `←` / `→` | Morceau précédent ou suivant |
| `←` / `→` | Reculer ou avancer de 5 s |
| `↑` / `↓` | Volume |
| `S` | Lecture aléatoire |
| `H` | Liker le morceau en cours |
| `F` | Plein écran |
| `L` | Paroles |
| `V` | Visuels, puis visuel suivant |
| `Q` | File d'attente |
| `D` | Téléchargements |
| `/` | Chercher dans la bibliothèque |
| `N` | Ouvrir Navidrome |
| `Échap` | Fermer, quitter le plein écran |

## Afficher ta musique sur Discord

C'est désactivé tant que tu ne l'as pas configuré.

1. Sur [discord.com/developers](https://discord.com/developers/applications), crée une application nommée « Onde ».
2. Copie son **Application ID**.
3. Dans Onde, va dans **Réglages → Discord** et clique sur **Coller l'identifiant**.

Discord doit être ouvert sur la même machine.

## Ta vie privée

Onde n'a ni compte, ni pisteur, ni télémétrie. Seules ces fonctions contactent des services, et les trois premières sont liées à Discord :

| Quand | Ce qui part | Vers |
|---|---|---|
| Discord activé | Titre, artiste, album | Discord |
| Recherche de pochette pour Discord | Artiste, album, titre | Apple Music, puis Deezer |
| Pochette introuvable | La pochette seule, réduite, pendant 24 h | litterbox.catbox.moe |
| « Héberger toutes les pochettes » activé | Toutes les pochettes, réduites, **sans suppression possible** | catbox.moe |
| Recherche et téléchargement | Ce que tu cherches | La source choisie |

Les images hébergées sont visibles par toute personne qui a leur lien.

## Compatibilité

| Système | État |
|---|---|
| Linux, Wayland (Hyprland) | ✅ Utilisé au quotidien |
| Linux, X11 | ❓ Pas testé |
| Windows | ❌ Le code utilise des fonctions propres à Unix |
| macOS | ❌ Pas pris en charge |

## Soutenir

Si Onde te rend service, [un café](https://ko-fi.com/forsakengh) m'aide à continuer.

---

<div align="center">
<sub>Polices sous licence SIL OFL, voir <code>assets/fonts</code>.</sub>
</div>
