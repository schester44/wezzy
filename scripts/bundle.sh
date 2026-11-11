#!/usr/bin/env bash
# Build Wezzy.app.
#
#   scripts/bundle.sh          # release build, ad-hoc signed -> target/release/Wezzy.app
#                              # (runs on this Mac only; never auto-updates)
#   scripts/bundle.sh --dist   # Developer ID + hardened runtime, notarized and
#                              # stapled -> target/release/dist/:
#                              #   Wezzy-<v>.dmg   for people to download
#                              #   Wezzy-<v>.zip   what the in-app updater installs
#                              #   latest.json     the update feed (see src/update.rs)
#
# Env:
#   SIGN_IDENTITY   "Developer ID Application: …" (--dist; default: the first one
#                   in the keychain)
#   NOTARY_PROFILE  notarytool keychain profile (--dist; default: dottt)
#   RELEASE_REPO    GitHub repo the release assets live in (default: schester44/wezzy)
#   MUX_FROM_WEZTERM=1  copy wezterm-mux-server from /Applications/WezTerm.app
#                       instead of building it (fast; must be the same version)
#
# Releasing is scripts/release.sh, which runs --dist and uploads the result.
set -euo pipefail
cd "$(dirname "$0")/.."

DIST=0
case "${1:-}" in
  "") ;;
  --dist) DIST=1 ;;
  *) echo "usage: $0 [--dist]" >&2; exit 2 ;;
esac

APP_NAME=Wezzy
BUNDLE_ID=dev.wezzy.app
WEZTERM_REV=$(grep -m1 -o 'rev = "[0-9a-f]*"' Cargo.toml | cut -d'"' -f2)
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
RELEASE_REPO=${RELEASE_REPO:-schester44/wezzy}
OUT=target/release/$APP_NAME.app
MUX_ROOT=target/mux-server/$WEZTERM_REV
ENTITLEMENTS=assets/macos/entitlements.plist

if [[ $DIST == 1 ]]; then
  IDENTITY=${SIGN_IDENTITY:-$(security find-identity -v -p codesigning \
    | grep -o '"Developer ID Application: [^"]*"' | head -1 | tr -d '"')}
  NOTARY_PROFILE=${NOTARY_PROFILE:-dottt}
  if [[ -z $IDENTITY ]]; then
    echo "error: no 'Developer ID Application' certificate in the keychain" >&2
    exit 1
  fi
  if ! xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
    echo "error: notarytool profile '$NOTARY_PROFILE' doesn't work; set NOTARY_PROFILE" >&2
    exit 1
  fi
else
  IDENTITY=-
fi

echo "==> wezzy $VERSION (wezterm $WEZTERM_REV)"
cargo build --release

if [[ "${MUX_FROM_WEZTERM:-}" == 1 ]]; then
  MUX_BIN=/Applications/WezTerm.app/Contents/MacOS/wezterm-mux-server
else
  MUX_BIN=$MUX_ROOT/bin/wezterm-mux-server
  if [[ ! -x $MUX_BIN ]]; then
    echo "==> building wezterm-mux-server (first time only)"
    cargo install --locked --git https://github.com/wezterm/wezterm --rev "$WEZTERM_REV" \
      --root "$MUX_ROOT" wezterm-mux-server
  fi
fi

echo "==> assembling $OUT"
rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp target/release/wezzy "$OUT/Contents/MacOS/wezzy"
cp "$MUX_BIN" "$OUT/Contents/MacOS/wezterm-mux-server"
cp assets/icon/wezzy.icns "$OUT/Contents/Resources/wezzy.icns"

# Programs run in a terminal ask for permissions in the terminal's name; macOS
# needs a reason string for each or it denies them without asking.
usage() { echo "  <key>NS${1}UsageDescription</key><string>A program running in $APP_NAME wants to $2.</string>"; }
USAGE=$(
  usage AppleEvents "control another app"
  usage Calendars "access your calendars"
  usage Camera "use the camera"
  usage Contacts "access your contacts"
  usage LocationAlways "use your location"
  usage Location "use your location"
  usage LocationWhenInUse "use your location"
  usage Microphone "use the microphone"
  usage SpeechRecognition "use speech recognition"
  usage Reminders "access your reminders"
  usage DocumentsFolder "access your Documents folder"
  usage DownloadsFolder "access your Downloads folder"
  usage DesktopFolder "access your Desktop folder"
  usage SystemAdministration "make administrative changes"
  usage LocalNetwork "access the local network"
  usage Bluetooth "use Bluetooth"
  usage BluetoothAlways "use Bluetooth"
  usage PhotoLibrary "access your photos"
)
cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>wezzy</string>
  <key>CFBundleIconFile</key><string>wezzy</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
$USAGE
</dict>
</plist>
PLIST
plutil -lint "$OUT/Contents/Info.plist" >/dev/null

echo "==> signing (${IDENTITY})"
if [[ $DIST == 1 ]]; then
  SIGN_ARGS=(--force --timestamp --options runtime --entitlements "$ENTITLEMENTS" --sign "$IDENTITY")
else
  SIGN_ARGS=(--force --timestamp=none --sign -)
fi
codesign "${SIGN_ARGS[@]}" "$OUT/Contents/MacOS/wezterm-mux-server"
codesign "${SIGN_ARGS[@]}" "$OUT"
codesign --verify --strict "$OUT" && echo "    signature ok"

if [[ $DIST == 0 ]]; then
  echo "==> done: $OUT"
  exit 0
fi

notarize() {
  echo "==> notarizing $(basename "$1")"
  local log
  log=$(xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait 2>&1) || true
  echo "$log" | sed 's/^/    /'
  if ! grep -q "status: Accepted" <<<"$log"; then
    echo "error: notarization failed; see: xcrun notarytool log <id> --keychain-profile $NOTARY_PROFILE" >&2
    exit 1
  fi
}

DIR=target/release/dist
ZIP=$DIR/$APP_NAME-$VERSION.zip
DMG=$DIR/$APP_NAME-$VERSION.dmg
rm -rf "$DIR"
mkdir -p "$DIR"

# The app is notarized on its own (as a zip) so its ticket can be stapled
# into the bundle: the updater installs the zip, and Gatekeeper then needs no
# network to accept it.
ditto -c -k --keepParent "$OUT" "$ZIP"
notarize "$ZIP"
xcrun stapler staple "$OUT"
rm -f "$ZIP"
ditto -c -k --keepParent "$OUT" "$ZIP"

echo "==> $DMG"
STAGE=$(mktemp -d)
cp -R "$OUT" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO -quiet "$DMG"
rm -rf "$STAGE"
codesign --force --timestamp --sign "$IDENTITY" "$DMG"
notarize "$DMG"
xcrun stapler staple "$DMG"
spctl --assess --type open --context context:primary-signature -v "$DMG"
spctl --assess --type execute -v "$OUT"

cat > "$DIR/latest.json" <<JSON
{
  "version": "$VERSION",
  "wezterm": "$WEZTERM_REV",
  "url": "https://github.com/$RELEASE_REPO/releases/download/v$VERSION/$APP_NAME-$VERSION.zip",
  "sha256": "$(shasum -a 256 "$ZIP" | cut -d' ' -f1)"
}
JSON

echo "==> done: $DIR"
command ls -lh "$DIR" | awk 'NR>1 {print "    " $5 "  " $9}'
