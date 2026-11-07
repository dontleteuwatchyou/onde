#!/bin/sh
# Signe le binaire Windows construit par la CI pour le commit courant.
#   scripts/sign-windows.sh 0.2.0             signe, sans rien publier
#   scripts/sign-windows.sh 0.2.0 --publish   l'ajoute à la release v0.2.0
set -eu

version=${1:?Utilisation : scripts/sign-windows.sh VERSION [--publish]}
publish=${2:-}
key=${ONDE_SIGNING_KEY:-$HOME/.config/onde-release/onde.key}
binary=onde-windows-x86_64.exe
root=$(git rev-parse --show-toplevel)
out=$root/target/dist/$version
mkdir -p "$out"
cd "$out"
rm -f "$binary"

run=$(gh run list -R dontleteuwatchyou/onde --workflow windows.yml --commit "$(git -C "$root" rev-parse HEAD)" \
    --status success --limit 1 --json databaseId --jq '.[0].databaseId')
[ -n "$run" ] || { echo "Erreur : aucune CI Windows réussie pour ce commit." >&2; exit 1; }
gh run download "$run" -R dontleteuwatchyou/onde --name onde-windows-x86_64 --dir .

sha256sum "$binary" >"$binary.sha256"
rsign sign -W -s "$key" -x "$binary.minisig" -t "onde $version $binary" "$binary"
rsign verify -p "$root/assets/update.pub" -x "$binary.minisig" "$binary"

if [ "$publish" = --publish ]; then
    gh release upload "v$version" "$binary" "$binary.sha256" "$binary.minisig"
else
    echo "Prêt dans $out. Relance avec --publish pour l'ajouter à la release."
fi
