#!/bin/sh
# Installe la dernière version d'Onde pour l'utilisateur courant, sans sudo.
#   sh install.sh               installe ou met à jour
#   sh install.sh --uninstall   retire Onde (ta musique et tes réglages restent)
set -eu

REPO=dontleteuwatchyou/onde
BIN_DIR=${ONDE_BIN_DIR:-$HOME/.local/bin}
APP_DIR=${XDG_DATA_HOME:-$HOME/.local/share}/applications

fail() {
    echo "Erreur : $1" >&2
    exit 1
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN_DIR/onde" "$APP_DIR/onde.desktop"
    echo "Onde est retiré. Ta musique et tes réglages n'ont pas bougé."
    exit 0
fi

[ "$(uname -s)" = Linux ] || fail "Onde ne fonctionne que sous Linux."
[ "$(uname -m)" = x86_64 ] || fail "seule la version x86_64 est compilée. Compile Onde depuis les sources."
command -v sha256sum >/dev/null || fail "sha256sum est introuvable."
command -v tar >/dev/null || fail "tar est introuvable."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
cd "$work"

# GitHub redirige /releases/latest vers .../releases/tag/<version>.
latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest" 2>/dev/null || true)
tag=${latest##*/tag/}

if [ "$tag" != "$latest" ] && [ -n "$tag" ]; then
    archive="onde-${tag#v}-linux-x86_64.tar.gz"
    echo "Téléchargement d'Onde $tag…"
    for file in "$archive" "$archive.sha256"; do
        curl -fsSL -o "$file" "https://github.com/$REPO/releases/download/$tag/$file" ||
            fail "le téléchargement de $file a échoué."
    done
elif command -v gh >/dev/null && gh release download -R "$REPO" --pattern 'onde-*-linux-x86_64.tar.gz*'; then
    echo "Onde est téléchargé avec GitHub CLI."
else
    fail "impossible de télécharger Onde. Le dépôt est privé : connecte-toi avec « gh auth login » sur un compte qui y a accès."
fi

sha256sum -c --quiet onde-*-linux-x86_64.tar.gz.sha256 || fail "l'archive est abîmée."
tar xzf onde-*-linux-x86_64.tar.gz

mkdir -p "$BIN_DIR" "$APP_DIR"
install -m755 onde-*-linux-x86_64/onde "$BIN_DIR/onde"
cat >"$APP_DIR/onde.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Onde
Comment=Lecteur de musique
Exec=$BIN_DIR/onde
Terminal=false
Categories=AudioVideo;Audio;Player;
EOF

echo "Onde est installé dans $BIN_DIR/onde."

missing=$(ldd "$BIN_DIR/onde" 2>&1 | grep -E 'not found' || true)
if [ -n "$missing" ]; then
    echo "Attention, il manque des bibliothèques sur ce système :" >&2
    echo "$missing" >&2
    echo "Voir la section Installation du README, ou compile depuis les sources." >&2
fi

case ":$PATH:" in
*":$BIN_DIR:"*) ;;
*) echo "Ajoute $BIN_DIR à ton PATH pour lancer la commande « onde »." ;;
esac
command -v yt-dlp >/dev/null || echo "En option : installe yt-dlp et ffmpeg pour télécharger depuis YouTube et SoundCloud."
