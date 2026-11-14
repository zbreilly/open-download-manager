#!/bin/bash
# Have Mozilla sign the Firefox extension, so that release Firefox installs it.
# Builds the extension, sends extension/dist/firefox to addons.mozilla.org on
# the unlisted channel (signed for handing out yourself, no public listing) and
# keeps the signed copy as extension/signed/odm-firefox-<version>.xpi. From
# then on build.sh puts that copy in extension/dist/odm-firefox.xpi.
#
# Needs an addons.mozilla.org account and its API credentials, from
# https://addons.mozilla.org/developers/addon/api/key/ :
#   export WEB_EXT_API_KEY=<JWT issuer>
#   export WEB_EXT_API_SECRET=<JWT secret>
#
# Mozilla signs each version number once: raise the version in both manifests
# (and the app's, see scripts/ci/check-versions.sh) before signing again.
set -euo pipefail
cd "$(dirname "$0")"
WEB_EXT="web-ext@10.8.0"

if [ -z "${WEB_EXT_API_KEY:-}" ] || [ -z "${WEB_EXT_API_SECRET:-}" ]; then
  echo "set WEB_EXT_API_KEY and WEB_EXT_API_SECRET (https://addons.mozilla.org/developers/addon/api/key/)" >&2
  exit 2
fi
command -v npx >/dev/null || { echo "npx not found: install Node.js 20 or later" >&2; exit 1; }

version=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' manifests/firefox.json | head -1)
signed="signed/odm-firefox-$version.xpi"
if [ -f "$signed" ]; then
  echo "version $version is already signed: extension/$signed" >&2
  exit 1
fi

bash build.sh
# web-ext keeps its upload ticket here, so a run that timed out waiting for
# approval picks the same submission up again
work="signed/web-ext"
mkdir -p "$work"
rm -f "$work"/*.xpi
# the credentials reach web-ext through the environment, not the command line
npx --yes "$WEB_EXT" sign --channel unlisted --source-dir dist/firefox --artifacts-dir "$work"

xpi=$(ls "$work"/*.xpi 2>/dev/null | head -1)
[ -n "$xpi" ] || { echo "web-ext finished without a signed .xpi in extension/$work" >&2; exit 1; }
mv "$xpi" "$signed"
cp "$signed" dist/odm-firefox.xpi
echo "signed: extension/$signed (also extension/dist/odm-firefox.xpi)"
