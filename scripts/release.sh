#!/bin/sh
# Prépare une version d'Onde depuis un clone propre et la signe.
#   scripts/release.sh 0.2.0             construit et signe, sans rien publier
#   scripts/release.sh 0.2.0 --publish   crée aussi la release GitHub
# Il faut rsign (cargo install rsign2) et la clé privée, jamais dans le dépôt :
# ONDE_SIGNING_KEY, par défaut ~/.config/onde-release/onde.key
set -eu

version=${1:?Utilisation : scripts/release.sh VERSION [--publish]}
publish=${2:-}
key=${ONDE_SIGNING_KEY:-$HOME/.config/onde-release/onde.key}
root=$(git rev-parse --show-toplevel)
cd "$root"

fail() {
    echo "Erreur : $1" >&2
    exit 1
}

[ -f "$key" ] || fail "clé privée introuvable : $key"
command -v rsign >/dev/null || fail "rsign est introuvable : cargo install rsign2"
[ -z "$(git status --porcelain)" ] || fail "le dépôt a des modifications non commitées."
grep -q "^version = \"$version\"$" Cargo.toml || fail "Cargo.toml n'est pas en version $version."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM
git clone -q "$root" "$work/onde"
(
    cd "$work/onde"
    RUSTFLAGS="--remap-path-prefix=$HOME=/build --remap-path-prefix=$work/onde=/build/onde" \
        cargo build --release --locked
)

out=$root/target/dist/$version
name=onde-$version-linux-x86_64
binary=onde-linux-x86_64
rm -rf "$out"
mkdir -p "$out/$name/licences"
cp "$work/onde/target/release/onde" "$out/$binary"
strip "$out/$binary"
cp "$out/$binary" "$out/$name/onde"
cp "$work/onde/README.md" "$work/onde/LICENSE" "$work/onde/assets/onde.svg" "$out/$name/"
cp "$work/onde"/assets/fonts/OFL-*.txt "$out/$name/licences/"
cd "$out"
tar czf "$name.tar.gz" "$name"
sha256sum "$name.tar.gz" >"$name.tar.gz.sha256"
# Le commentaire signé empêche de faire passer un autre binaire pour cette version.
rsign sign -W -s "$key" -x "$binary.minisig" -t "onde $version $binary" "$binary"
rsign verify -p "$root/assets/update.pub" -x "$binary.minisig" "$binary"
rm -rf "$name"

if [ "$publish" = --publish ]; then
    gh release create "v$version" --target "$(git -C "$root" rev-parse HEAD)" --title "Onde $version" \
        --generate-notes "$name.tar.gz" "$name.tar.gz.sha256" "$binary" "$binary.minisig"
else
    echo "Prêt dans $out. Relance avec --publish pour créer la release."
fi
