#!/usr/bin/env bash
# Build Dekita.app (and optionally a DMG).
#
#   scripts/bundle.sh                 # release build -> target/release/Dekita.app
#   scripts/bundle.sh --dmg           # also target/release/Dekita-<version>.dmg
#
# Env:
#   SIGN_IDENTITY   "Developer ID Application: …" (default: ad-hoc "-", local use only)
#   NOTARY_PROFILE  notarytool keychain profile; with --dmg, notarizes + staples
#   MUX_FROM_WEZTERM=1  copy wezterm-mux-server from /Applications/WezTerm.app
#                       instead of building it (fast; must be the same version)
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME=Dekita
BUNDLE_ID=dev.dekita.app
WEZTERM_REV=$(grep -m1 -o 'rev = "[0-9a-f]*"' Cargo.toml | cut -d'"' -f2)
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
OUT=target/release/$APP_NAME.app
MUX_ROOT=target/mux-server/$WEZTERM_REV

echo "==> dekita $VERSION (wezterm $WEZTERM_REV)"
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
cp target/release/dekita "$OUT/Contents/MacOS/dekita"
cp "$MUX_BIN" "$OUT/Contents/MacOS/wezterm-mux-server"
cp assets/icon/dekita.icns "$OUT/Contents/Resources/dekita.icns"
cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>dekita</string>
  <key>CFBundleIconFile</key><string>dekita</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

IDENTITY=${SIGN_IDENTITY:--}
echo "==> signing (${IDENTITY})"
SIGN_ARGS=(--force --timestamp=none --sign "$IDENTITY")
[[ "$IDENTITY" != "-" ]] && SIGN_ARGS=(--force --timestamp --options runtime --sign "$IDENTITY")
codesign "${SIGN_ARGS[@]}" "$OUT/Contents/MacOS/wezterm-mux-server"
codesign "${SIGN_ARGS[@]}" "$OUT"
codesign --verify --strict "$OUT" && echo "    signature ok"

if [[ "${1:-}" == "--dmg" ]]; then
  DMG=target/release/$APP_NAME-$VERSION.dmg
  echo "==> $DMG"
  STAGE=$(mktemp -d)
  cp -R "$OUT" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$DMG"
  hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
  rm -rf "$STAGE"
  if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    echo "==> notarizing"
    xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
  fi
fi
echo "==> done: $OUT"
