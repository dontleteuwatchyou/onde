#!/bin/sh
# Installe la dernière version d'Onde pour l'utilisateur courant, sans sudo.
#   sh install.sh               installe ou met à jour
#   sh install.sh --uninstall   retire Onde (ta musique et tes réglages restent)
set -eu

REPO=dontleteuwatchyou/onde
BIN_DIR=${ONDE_BIN_DIR:-$HOME/.local/bin}
DATA_DIR=${XDG_DATA_HOME:-$HOME/.local/share}
APP_DIR=$DATA_DIR/applications
ICON_DIR=$DATA_DIR/icons/hicolor/scalable/apps

fail() {
    echo "Erreur : $1" >&2
    exit 1
}

case "${1:-}" in
"") ;;
--uninstall)
    rm -f "$BIN_DIR/onde" "$APP_DIR/onde.desktop" "$ICON_DIR/onde.svg"
    echo "Onde est retiré. Ta musique et tes réglages n'ont pas bougé."
    exit 0
    ;;
*) fail "option inconnue : $1. Utilise --uninstall ou rien." ;;
esac

[ "$(uname -s)" = Linux ] || fail "Onde ne fonctionne que sous Linux."
[ "$(uname -m)" = x86_64 ] || fail "seule la version x86_64 est compilée. Compile Onde depuis les sources."
for tool in curl sha256sum tar; do
    command -v "$tool" >/dev/null || fail "$tool est introuvable."
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
cd "$work"

# GitHub redirige /releases/latest vers .../releases/tag/<version>.
latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
    fail "impossible de joindre GitHub."
tag=${latest##*/tag/}
[ "$tag" != "$latest" ] && [ -n "$tag" ] || fail "aucune version publiée n'a été trouvée."

archive="onde-${tag#v}-linux-x86_64.tar.gz"
echo "Téléchargement d'Onde $tag…"
for file in "$archive" "$archive.sha256"; do
    curl -fsSL -o "$file" "https://github.com/$REPO/releases/download/$tag/$file" ||
        fail "le téléchargement de $file a échoué."
done

sha256sum -c --quiet "$archive.sha256" || fail "l'archive est abîmée."
tar xzf "$archive"

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR"
install -m755 "${archive%.tar.gz}/onde" "$BIN_DIR/onde"
# Les archives d'avant la 0.1.2 n'ont pas d'icône : sans elle, Onde marche quand même.
cp "${archive%.tar.gz}/onde.svg" "$ICON_DIR/onde.svg" 2>/dev/null ||
    curl -fsSL -o "$ICON_DIR/onde.svg" "https://raw.githubusercontent.com/$REPO/master/assets/onde.svg" ||
    echo "Icône non installée : Onde s'affichera sans dans le menu."
cat >"$APP_DIR/onde.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Onde
Comment=Lecteur de musique
Icon=onde
Exec=$BIN_DIR/onde
Terminal=false
StartupWMClass=onde
Categories=AudioVideo;Audio;Player;
DESKTOP

echo "Onde $tag est installé dans $BIN_DIR/onde."

missing=$(ldd "$BIN_DIR/onde" 2>&1 | grep 'not found' || true)
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
