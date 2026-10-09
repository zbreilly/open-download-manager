#!/bin/bash
# Assemble browser-extension dist bundles for Chromium and Firefox.
set -e
cd "$(dirname "$0")"
ROOT="$(cd .. && pwd)"          # repo root
BRAND="$(cd "$ROOT/../.." && pwd)/download-manager-ui/brand/dist/browser-extension"
ICONS_DIR="$BRAND"
TMP="$(mktemp -d)"

pack() {
  local target="$1" manifest="$2" out="$3"
  rm -rf "$TMP/$target"
  mkdir -p "$TMP/$target/icons"
  cp common/background.js common/capture.js common/popup.html common/popup.js \
     "$TMP/$target/"
  cp "$manifest" "$TMP/$target/manifest.json"
  # brand icons at the sizes the manifests reference
  for size in 16 32 48 96 128; do
    icon="$ICONS_DIR/icon-${size}.png"
    [ -f "$icon" ] && cp "$icon" "$TMP/$target/icons/${size}.png"
  done
  (cd "$TMP/$target" && zip -qr "$ROOT/extension/$out" .)
  echo "packed extension/$out"
}

pack chromium chromium/manifest.json odm-chromium.zip
pack firefox firefox/manifest.json odm-firefox.xpi

rm -rf "$TMP"
echo "done"
