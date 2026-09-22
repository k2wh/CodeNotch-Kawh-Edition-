#!/usr/bin/env bash
# Sign a release's installers and publish the manifest the updater reads.
#
#   bash packaging/sign-release.sh v0.2.0 [path to the key]
#
# Runs where the signing key is — the maintainer's machine — after the
# Release workflow has attached the installers. It signs each file the
# updater can install, writes `latest.json` from the release's own notes, and
# uploads both. Until it runs, installed copies find no update; after it, they
# find this one.
#
# The key is only ever handed to the Tauri CLI as a path: it is not read,
# printed or copied anywhere by this script. Losing it means no installed copy
# will accept an update again, so keep a copy somewhere safe.
set -euo pipefail

tag=${1:-}
key=${2:-$HOME/.tauri/codenotch.key}
repo=${CODENOTCH_REPO:-k2wh/CodeNotch-Kawh-Edition-}

if [ -z "$tag" ]; then
  echo "usage: bash packaging/sign-release.sh <tag> [path to the key]" >&2
  exit 2
fi
if [ ! -f "$key" ]; then
  echo "no signing key at $key" >&2
  echo "make one with: npx tauri signer generate -w $key --ci" >&2
  exit 1
fi
cd "$(dirname "$0")/.."

work=$(mktemp -d -t codenotch-sign-XXXXXX)
trap 'rm -rf "$work"' EXIT

echo "== fetching $tag from $repo"
gh release download "$tag" -R "$repo" -D "$work" --clobber \
  -p '*-setup.exe' -p '*.deb' -p '*.app.tar.gz'
ls -la "$work"

# Exactly one file per kind, or the manifest would point at the wrong one.
one() {
  local -a found=("$work"/$1)
  if [ ${#found[@]} -ne 1 ] || [ ! -f "${found[0]}" ]; then
    echo "expected one file matching $1 in the release, found ${#found[@]}" >&2
    exit 1
  fi
  printf '%s' "${found[0]}"
}
exe=$(one '*-setup.exe')
deb=$(one '*.deb')
app=$(one '*.app.tar.gz')

echo "== signing"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD-}"
for file in "$exe" "$deb" "$app"; do
  npx tauri signer sign -f "$key" -p "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" "$file" >/dev/null
  test -s "$file.sig" || { echo "no signature written for $file" >&2; exit 1; }
  echo "signed $(basename "$file")"
done

echo "== writing latest.json"
version=${tag#v}
base="https://github.com/$repo/releases/download/$tag"
notes=$(gh release view "$tag" -R "$repo" --json body --jq .body)
entry() {
  jq -n --arg url "$base/$(basename "$1")" --arg sig "$(cat "$1.sig")" \
    '{signature: $sig, url: $url}'
}
# The keys the updater asks for, `{os}-{arch}-{installer}` first and
# `{os}-{arch}` after it. Every platform has to be here: a missing one is an
# error to the app that asks, not "no update".
jq -n \
  --arg version "$version" \
  --arg notes "$notes" \
  --arg pub_date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson windows "$(entry "$exe")" \
  --argjson linux "$(entry "$deb")" \
  --argjson mac "$(entry "$app")" \
  '{version: $version, notes: $notes, pub_date: $pub_date, platforms: {
     "windows-x86_64-nsis": $windows,
     "windows-x86_64": $windows,
     "linux-x86_64-deb": $linux,
     "linux-x86_64": $linux,
     "darwin-aarch64-app": $mac,
     "darwin-aarch64": $mac,
     "darwin-x86_64-app": $mac,
     "darwin-x86_64": $mac
   }}' > "$work/latest.json"
jq '{version, platforms: (.platforms | keys)}' "$work/latest.json"

echo "== uploading"
# Signatures first, manifest last: until it lands, an installed copy sees no
# update rather than one it can't verify.
gh release upload "$tag" -R "$repo" --clobber \
  "$exe.sig" "$deb.sig" "$app.sig"
gh release upload "$tag" -R "$repo" --clobber "$work/latest.json"
gh release view "$tag" -R "$repo" --json assets --jq '.assets[] | "\(.name)  \(.size)"'
echo "== done: $tag can now update the copies already out there"
