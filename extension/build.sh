#!/bin/bash
# Assemble unpacked extensions in extension/dist/{chromium,firefox} and the
# files to install or upload:
#   odm-chromium.zip   for the Chrome Web Store / Edge Add-ons
#   odm-chromium.crx   signed, for Chromium browsers that take a .crx file
#   odm-firefox.xpi    unsigned, or the copy Mozilla signed for this version
#                      once sign-firefox.sh has run (release Firefox needs it)
#
# The .crx is signed with extension/key.pem, or the file ODM_CRX_KEY names.
# It is created on the first build; the extension's ID comes from it. CI makes
# no .crx unless it is given that key.
set -euo pipefail
cd "$(dirname "$0")"
ICONS="../download-manager-ui/brand/dist/browser-extension"
KEY="${ODM_CRX_KEY:-key.pem}"
rm -rf dist
for target in chromium firefox; do
  out="dist/$target"
  mkdir -p "$out/icons"
  cp src/*.js src/*.html src/*.css "$out/"
  cp "manifests/$target.json" "$out/manifest.json"
  cp "$ICONS"/icon-*.png "$out/icons/"
done
if command -v zip >/dev/null; then
  (cd dist/chromium && zip -qrX ../odm-chromium.zip . -x '*.DS_Store')
  (cd dist/firefox && zip -qrX ../odm-firefox.xpi . -x '*.DS_Store')
  version=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' manifests/firefox.json | head -1)
  if [ -f "signed/odm-firefox-$version.xpi" ]; then
    cp "signed/odm-firefox-$version.xpi" dist/odm-firefox.xpi
    echo "dist/odm-firefox.xpi: the copy Mozilla signed for $version"
  else
    echo "dist/odm-firefox.xpi: unsigned (sign-firefox.sh has Mozilla sign it)"
  fi
  if ! command -v node >/dev/null; then
    echo "node not found: skipped dist/odm-chromium.crx"
  elif [ -n "${CI:-}" ] && [ ! -f "$KEY" ]; then
    # a throwaway key would give every CI build a different extension ID
    echo "no signing key in CI: skipped dist/odm-chromium.crx"
  else
    node pack-crx.mjs dist/odm-chromium.zip "$KEY" dist/odm-chromium.crx
  fi
else
  echo "zip not found: skipped the .zip, .crx and .xpi"
fi
echo "built extension/dist (load dist/chromium unpacked, or dist/firefox via about:debugging)"
