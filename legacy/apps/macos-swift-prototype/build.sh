#!/bin/bash
# Build the native macOS shell (SwiftUI front-end for odm-serve).
set -e
cd "$(dirname "$0")"
mkdir -p build/OpenDM.app/Contents/MacOS
swiftc -parse-as-library -O OpenDownloadManager.swift \
  -o build/OpenDM.app/Contents/MacOS/OpenDM
cat > build/OpenDM.app/Contents/Info.plist <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Open Download Manager</string>
  <key>CFBundleIdentifier</key><string>app.opendm</string>
  <key>CFBundleExecutable</key><string>OpenDM</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>CFBundleIconFile</key><string>OpenDM</string>
</dict></plist>
PLIST
# brand icon (from download-manager-ui/brand/dist/app-icons)
ICON_SRC=""
for c in "../../../../download-manager-ui/brand/dist/app-icons/icon.icns"; do
  [ -f "$c" ] && ICON_SRC="$c" && break
done
if [ -n "$ICON_SRC" ]; then
  mkdir -p build/OpenDM.app/Contents/Resources
  cp "$ICON_SRC" build/OpenDM.app/Contents/Resources/OpenDM.icns
fi
# bundle the odm-serve release binary (the app launches it on start)
RUST_BIN="../../rust/target/release/odm-serve"
if [ -f "$RUST_BIN" ]; then
  cp "$RUST_BIN" build/OpenDM.app/Contents/Resources/odm-serve
  chmod +x build/OpenDM.app/Contents/Resources/odm-serve
else
  echo "WARNING: odm-serve release binary not found at $RUST_BIN — build it first"
fi

# brand tray templates (macOS)
TRAY_SRC=""
for c in "../../../../download-manager-ui/brand/dist/tray/macos"; do
  [ -d "$c" ] && TRAY_SRC="$c" && break
done
if [ -n "$TRAY_SRC" ]; then
  cp "$TRAY_SRC"/tray-*Template*.png build/OpenDM.app/Contents/Resources/ 2>/dev/null || true
fi
touch build/OpenDM.app/Contents/MacOS/OpenDM
chmod +x build/OpenDM.app/Contents/MacOS/OpenDM
# Codesign: prefer Developer ID Application + hardened runtime; fall back
# to ad-hoc so local builds always work.
IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null \
  | grep "Developer ID Application" | head -1 \
  | awk '{print $2}')
if [ -n "$IDENTITY" ]; then
  # inside-out signing: nested binaries first (Rust builds come out ad-hoc
  # linker-signed; --deep alone does not re-sign them), then the bundle
  codesign -s "$IDENTITY" --force --timestamp --options runtime \
    build/OpenDM.app/Contents/Resources/odm-serve \
    && echo "codesigned nested odm-serve" \
    || echo "ERROR: nested binary signing failed"
  codesign -s "$IDENTITY" --force --timestamp \
    --options runtime build/OpenDM.app \
    && echo "codesigned (Developer ID): $IDENTITY" \
    || echo "ERROR: Developer ID signing failed"
else
  codesign -s - --force --deep build/OpenDM.app 2>/dev/null \
    && echo "ad-hoc codesigned (no Developer ID cert in keychain)" \
    || echo "codesign skipped"
fi

echo "built build/OpenDM.app (bundled server + signature)"
