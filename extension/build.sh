#!/bin/bash
# Assemble unpacked extensions in extension/dist/{chromium,firefox} plus
# a .zip (Chromium) and .xpi (Firefox, unsigned) for each.
set -euo pipefail
cd "$(dirname "$0")"
ICONS="../download-manager-ui/brand/dist/browser-extension"
rm -rf dist
for target in chromium firefox; do
  out="dist/$target"
  mkdir -p "$out/icons"
  cp src/*.js src/*.html src/*.css "$out/"
  cp "manifests/$target.json" "$out/manifest.json"
  cp "$ICONS"/icon-*.png "$out/icons/"
done
if command -v zip >/dev/null; then
  (cd dist/chromium && zip -qr ../odm-chromium.zip .)
  (cd dist/firefox && zip -qr ../odm-firefox.xpi .)
fi
echo "built extension/dist (load dist/chromium unpacked, or dist/firefox via about:debugging)"
