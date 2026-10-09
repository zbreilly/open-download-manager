#!/bin/bash
# Build the Open Download Manager .dmg installer:
# drag-to-Applications layout with the brand background.
set -e
cd "$(dirname "$0")"

if [ "${1:-}" = "--check" ]; then
  # poll-only mode: NEVER rebuilds/re-signs here (that would invalidate the
  # pending ticket) — checks status, staples when Apple accepts
  DMG_OUT="dist/OpenDownloadManager-0.1.0.dmg"
  ID=$(cat dist/notary-submission.txt 2>/dev/null)
  [ -n "$ID" ] || { echo "no submission recorded"; exit 1; }
  xcrun notarytool info "$ID" --keychain-profile ODM_NOTARY | grep -E 'status|id'
  if xcrun notarytool info "$ID" --keychain-profile ODM_NOTARY 2>/dev/null | grep -q "Accepted"; then
    xcrun stapler staple "$DMG_OUT" && xcrun stapler validate "$DMG_OUT" \
      && echo "staple: OK — DMG is fully notarized" \
      && spctl -a -t open --context context:primary-signature -v "$DMG_OUT" || true
  else
    echo "still processing at Apple — check again in a minute"
  fi
  exit 0
fi

REPO="$(cd ../.. && pwd)"
BRAND=""
for c in "$(cd "$REPO/../.." && pwd)/download-manager-ui/brand/dist"; do
  [ -d "$c" ] && BRAND="$c" && break
done
[ -n "$BRAND" ] || { echo "brand package not found"; exit 1; }
APP="build/OpenDM.app"
VOL="Open Download Manager"
DMG_RW="build/odm-rw.dmg"
DMG_OUT="dist/OpenDownloadManager-0.1.0.dmg"

[ -d "$APP" ] || ./build.sh

BG="$BRAND/installer/dmg-background.png"
[ -f "$BG" ] || BG="$BRAND/installer/dmg-background@2x.png"
[ -f "$BG" ] || { echo "no dmg background found in brand package"; exit 1; }

mkdir -p dist
rm -f "$DMG_RW" "$DMG_OUT"

# 1) blank read-write image
hdiutil create -size 120m -fs "HFS+" -volname "$VOL" "$DMG_RW" -attach -plist > /tmp/odm_dmg_mount.plist
MOUNT=$(python3 -c "
import plistlib
d = plistlib.load(open('/tmp/odm_dmg_mount.plist','rb'))
for e in d['system-entities']:
    if e.get('mount-point'): print(e['mount-point'])
")
[ -n "$MOUNT" ] || { echo "mount failed"; exit 1; }
echo "mounted at: $MOUNT"

cleanup() { hdiutil detach "$MOUNT" -force >/dev/null 2>&1 || true; }
trap cleanup ERR

# 2) populate: app + Applications symlink + hidden background + volume icon
cp -R "$APP" "$MOUNT/OpenDM.app"
ln -sfn /Applications "$MOUNT/Applications"
mkdir -p "$MOUNT/.background"
cp "$BG" "$MOUNT/.background/bg.png"
cp build/OpenDM.app/Contents/Resources/OpenDM.icns "$MOUNT/.VolumeIcon.icns" 2>/dev/null || true

# 3) Finder view: 660x400 window, 128px icons, background, app left / Applications right
osascript <<APPLESCRIPT || echo "WARN: Finder view options failed (DMG still valid, plain layout)"
with timeout of 30 seconds
  tell application "Finder"
    tell disk "Open Download Manager"
      open
      delay 1
      set current view of container window to icon view
      set toolbar visible of container window to false
      set statusbar visible of container window to false
      set the bounds of container window to {200, 120, 860, 520}
      set theViewOptions to the icon view options of the container window
      set arrangement of theViewOptions to not arranged
      set icon size of theViewOptions to 128
      try
        set background picture of theViewOptions to file "bg.png" of folder ".background" of disk "Open Download Manager"
      end try
      set position of item "OpenDM.app" of the container window to {170, 195}
      set position of item "Applications" of the container window to {490, 195}
      update without registering applications
      delay 2
      close
    end tell
  end tell
end timeout
APPLESCRIPT

# make Finder honor the .DS_Store on mount
bless --folder "$MOUNT" --openfolder "$MOUNT" 2>/dev/null || true
# volume custom icon flag (SetFile is deprecated and flaky with spaces —
# verify existence first, ignore failures: purely cosmetic)
if [ -f "$MOUNT/.VolumeIcon.icns" ]; then
  SetFile -a C "$MOUNT/.VolumeIcon.icns" 2>/dev/null || echo "note: SetFile skipped (deprecated tool)"
else
  echo "note: volume icon not copied (cosmetic only)"
fi

# 4) detach + convert to compressed read-only
hdiutil detach "$MOUNT" -force
trap - ERR
hdiutil convert "$DMG_RW" -format UDZO -imagekey zlib-level=9 -o "$DMG_OUT"
rm -f "$DMG_RW"

# 5) sign the DMG itself if a Developer ID is present
IDENTITY=$(security find-identity -v -p codesigning 2>/dev/null \
  | grep "Developer ID Application" | head -1 \
  | awk '{print $2}')
if [ -n "$IDENTITY" ]; then
  codesign -s "$IDENTITY" --timestamp "$DMG_OUT" \
    && echo "DMG signed (Developer ID)"
else
  echo "note: DMG unsigned (no Developer ID cert) — fine for local use"
fi

# 6) notarize + staple if notarytool credentials are stored
#    (one-time: xcrun notarytool store-credentials ODM_NOTARY \
#       --apple-id <id> --team-id <TEAM> --password <app-specific-pw>)
if [ -z "$IDENTITY" ]; then
  echo "note: skipping notarization — no Developer ID cert in keychain (build is ad-hoc signed)."
  echo "      create it at developer.apple.com -> Certificates -> Developer ID Application,"
  echo "      install the .cer, then re-run."
elif xcrun notarytool history --keychain-profile ODM_NOTARY >/dev/null 2>&1; then
  # non-blocking: submit, record the id, return immediately
  xcrun notarytool submit "$DMG_OUT" --keychain-profile ODM_NOTARY 2>&1 | tee dist/notary-submit.log | grep -E 'id:|path:'
  grep -E '^  id:' dist/notary-submit.log | awk '{print $2}' | head -1 > dist/notary-submission.txt
  echo ""
  SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
  echo "submitted (non-blocking). when you want to finish:"
  echo "  \"$SCRIPT_DIR/build-dmg.sh\" --check    # staples automatically once Apple accepts"
else
  echo "note: notarytool profile 'ODM_NOTARY' not stored — skipping notarization."
  echo "      one-time setup:"
  echo "        xcrun notarytool store-credentials ODM_NOTARY --apple-id <id> --team-id <TEAM> --password <app-specific-pw>"
fi

# 7) verify image integrity
hdiutil verify "$DMG_OUT"
echo "built: $DMG_OUT ($(du -h "$DMG_OUT" | cut -f1))"
