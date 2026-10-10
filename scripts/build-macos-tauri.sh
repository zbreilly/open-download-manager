#!/bin/bash
# Build the Tauri version of Open Download Manager for macOS: .app (+ .dmg),
# signed with your Developer ID when one is in the keychain (or the identity
# in ODM_SIGN_IDENTITY), ad-hoc otherwise. The native app is built by
# scripts/build-macos.sh instead.
#
#   scripts/build-macos-tauri.sh               # .app + .dmg for this Mac's CPU
#   scripts/build-macos-tauri.sh --install     # ...and copy the app to /Applications, then open it
#   scripts/build-macos-tauri.sh --universal   # Apple silicon + Intel in one binary
#   scripts/build-macos-tauri.sh --notarize    # submit the DMG to Apple (keychain profile ODM_NOTARY) and staple
#   scripts/build-macos-tauri.sh --no-dmg      # .app only (fastest)
#
# Needs: Rust (rustup) and Xcode Command Line Tools. Node is optional: the
# prebuilt frontend in ui/dist is used when npm is not installed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="Open Download Manager"
BIN="open-download-manager"
BUNDLE_ID="app.opendm.desktop"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
NOTARY_PROFILE="${ODM_NOTARY_PROFILE:-ODM_NOTARY}"

UNIVERSAL=0; DMG=1; NOTARIZE=0; INSTALL=0
for a in "$@"; do
  case "$a" in
    --universal) UNIVERSAL=1 ;;
    --no-dmg) DMG=0 ;;
    --notarize) NOTARIZE=1 ;;
    --install) INSTALL=1 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown option: $a"; exit 2 ;;
  esac
done

step() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "this script builds the macOS app and must run on a Mac"
command -v cargo >/dev/null || { [ -x "$HOME/.cargo/bin/cargo" ] && export PATH="$HOME/.cargo/bin:$PATH"; }
command -v cargo >/dev/null || die "Rust is not installed. Install it with:  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
RUSTV="$(rustc --version | awk '{print $2}')"
if [ "$(printf '%s\n1.90.0\n' "$RUSTV" | sort -V | head -1)" != "1.90.0" ]; then
  if command -v rustup >/dev/null; then
    echo "Rust $RUSTV is older than 1.90 — updating the stable toolchain"
    rustup update stable
  else
    die "Rust $RUSTV is too old (need 1.90+). Update it with: rustup update stable"
  fi
fi
xcode-select -p >/dev/null 2>&1 || die "Xcode Command Line Tools missing. Run:  xcode-select --install"

# ---------------------------------------------------------------- frontend
step "Frontend"
if command -v npm >/dev/null 2>&1; then
  ( cd ui
    if [ ! -d node_modules ]; then npm ci --no-audit --no-fund || npm install --no-audit --no-fund; fi
    npm run build )
elif [ -f ui/dist/index.html ]; then
  echo "npm not found — using the prebuilt frontend in ui/dist"
else
  die "ui/dist is missing and npm is not installed (brew install node)"
fi

step "Browser extension"
bash extension/build.sh

# ---------------------------------------------------------------- compile
build_target() {
  local t="$1"
  if [ -n "$t" ]; then
    rustup target add "$t" >/dev/null 2>&1 || true
    cargo build --release -p "$BIN" --features custom-protocol --target "$t"
  else
    cargo build --release -p "$BIN" --features custom-protocol
  fi
}

step "Compiling (release)"
export MACOSX_DEPLOYMENT_TARGET=11.0
if [ "$UNIVERSAL" = 1 ]; then
  build_target aarch64-apple-darwin
  build_target x86_64-apple-darwin
  mkdir -p target/universal-apple-darwin/release
  lipo -create -output "target/universal-apple-darwin/release/$BIN" \
    "target/aarch64-apple-darwin/release/$BIN" "target/x86_64-apple-darwin/release/$BIN"
  EXE="target/universal-apple-darwin/release/$BIN"
else
  build_target ""
  EXE="target/release/$BIN"
fi
[ -x "$EXE" ] || die "binary not found at $EXE"

# ---------------------------------------------------------------- bundle
step "Assembling $APP_NAME.app"
OUT="dist/macos"
APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$EXE" "$APP/Contents/MacOS/$BIN"
cp src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"
if [ -d extension/dist ]; then
  mkdir -p "$APP/Contents/Resources/extension"
  cp -R extension/dist/. "$APP/Contents/Resources/extension/"
fi
BUILD_NUM="$(date +%Y%m%d%H%M)"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$BIN</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD_NUM</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSRequiresAquaSystemAppearance</key><false/>
  <key>NSHumanReadableCopyright</key><string>© 2026 Open Download Manager</string>
  <key>NSAppleEventsUsageDescription</key><string>Open Download Manager can shut down your Mac when a queue finishes, if you turn that on in the scheduler.</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
      <key>CFBundleURLSchemes</key><array><string>odm</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST
printf 'APPL????' > "$APP/Contents/PkgInfo"

# ---------------------------------------------------------------- sign
step "Code signing"
IDENTITY="${ODM_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null | grep 'Developer ID Application' | head -1 | awk '{print $2}' || true)}"
# the hardened runtime needs the Apple Events entitlement for "shut down
# when the queue finishes" (osascript → System Events)
ENTITLEMENTS="apps/macos/entitlements.plist"
if [ -n "$IDENTITY" ]; then
  codesign --force --timestamp --options runtime --entitlements "$ENTITLEMENTS" -s "$IDENTITY" "$APP/Contents/MacOS/$BIN"
  codesign --force --timestamp --options runtime --entitlements "$ENTITLEMENTS" -s "$IDENTITY" "$APP"
  echo "signed with Developer ID ($IDENTITY)"
else
  codesign --force -s - "$APP/Contents/MacOS/$BIN"
  codesign --force -s - "$APP"
  echo "ad-hoc signed (no Developer ID certificate in the keychain) — fine for this Mac"
fi
codesign --verify --strict "$APP" && echo "signature OK"

# ---------------------------------------------------------------- dmg
if [ "$DMG" = 1 ]; then
  step "Building DMG"
  DMG_OUT="$OUT/OpenDownloadManager-$VERSION.dmg"
  DMG_RW="$OUT/odm-rw.dmg"
  VOL="$APP_NAME"
  BG="src-tauri/installer/dmg-background.png"
  rm -f "$DMG_OUT" "$DMG_RW"
  # detach a stale mount from an earlier run
  [ -d "/Volumes/$VOL" ] && hdiutil detach "/Volumes/$VOL" -force >/dev/null 2>&1 || true
  SIZE_MB=$(( $(du -sm "$APP" | cut -f1) + 30 ))
  hdiutil create -size "${SIZE_MB}m" -fs HFS+ -volname "$VOL" "$DMG_RW" >/dev/null
  MOUNT="$(hdiutil attach "$DMG_RW" -nobrowse -noautoopen | grep -o '/Volumes/.*' | head -1)"
  [ -n "$MOUNT" ] || die "could not mount the DMG"
  trap 'hdiutil detach "$MOUNT" -force >/dev/null 2>&1 || true' EXIT
  cp -R "$APP" "$MOUNT/"
  ln -s /Applications "$MOUNT/Applications"
  mkdir -p "$MOUNT/.background"
  cp "$BG" "$MOUNT/.background/bg.png"
  cp src-tauri/icons/icon.icns "$MOUNT/.VolumeIcon.icns"
  # Finder layout is cosmetic; a failure (no Automation permission) still yields a valid DMG
  hdiutil detach "$MOUNT" -force >/dev/null
  MOUNT="$(hdiutil attach "$DMG_RW" -noautoopen | grep -o '/Volumes/.*' | head -1)"
  osascript <<APPLESCRIPT >/dev/null 2>&1 || echo "note: Finder layout skipped (grant Terminal Automation access for the styled window)"
with timeout of 30 seconds
  tell application "Finder"
    tell disk "$VOL"
      open
      delay 1
      set current view of container window to icon view
      set toolbar visible of container window to false
      set statusbar visible of container window to false
      set the bounds of container window to {200, 120, 860, 520}
      set opts to the icon view options of container window
      set arrangement of opts to not arranged
      set icon size of opts to 128
      set background picture of opts to file ".background:bg.png"
      set position of item "$APP_NAME.app" of container window to {180, 215}
      set position of item "Applications" of container window to {480, 215}
      update without registering applications
      delay 1
      close
    end tell
  end tell
end timeout
APPLESCRIPT
  SetFile -a C "$MOUNT" 2>/dev/null || true
  sync
  hdiutil detach "$MOUNT" -force >/dev/null
  trap - EXIT
  hdiutil convert "$DMG_RW" -format UDZO -imagekey zlib-level=9 -o "$DMG_OUT" >/dev/null
  rm -f "$DMG_RW"
  if [ -n "$IDENTITY" ]; then codesign --force --timestamp -s "$IDENTITY" "$DMG_OUT"; fi
  echo "built $DMG_OUT ($(du -h "$DMG_OUT" | cut -f1))"

fi

# ---------------------------------------------------------------- notarize
if [ "$NOTARIZE" = 1 ]; then
  step "Notarizing (profile $NOTARY_PROFILE)"
  [ -n "$IDENTITY" ] || die "notarization needs a Developer ID Application certificate"
  if [ "$DMG" = 1 ]; then
    xcrun notarytool submit "$DMG_OUT" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 30m
    xcrun stapler staple "$DMG_OUT"
    spctl -a -t open --context context:primary-signature -v "$DMG_OUT" || true
  else
    ZIP="$OUT/odm-notarize.zip"
    ditto -c -k --keepParent "$APP" "$ZIP"
    xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait --timeout 30m
    rm -f "$ZIP"
  fi
  xcrun stapler staple "$APP" || true
fi

# ---------------------------------------------------------------- install
if [ "$INSTALL" = 1 ]; then
  step "Installing to /Applications"
  osascript -e "tell application \"$APP_NAME\" to quit" >/dev/null 2>&1 || true
  sleep 1
  rm -rf "/Applications/$APP_NAME.app"
  cp -R "$APP" /Applications/
  open "/Applications/$APP_NAME.app"
  echo "installed and launched /Applications/$APP_NAME.app"
fi

step "Done"
echo "app: $ROOT/$APP"
[ "$DMG" = 1 ] && echo "dmg: $ROOT/$OUT/OpenDownloadManager-$VERSION.dmg"
exit 0
