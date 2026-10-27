#!/usr/bin/env bash
# Build, bundle and code-sign Rile.app (optionally notarize + DMG).
#
# Env:
#   SIGN_IDENTITY   codesign identity (default: first "Developer ID Application" in keychain; "-" = ad-hoc)
#   BUNDLE_ID       default dev.rile.app
#   TARGET          e.g. aarch64-apple-darwin | x86_64-apple-darwin | universal (default: host)
#   NOTARY_PROFILE  notarytool keychain profile; if set, notarizes + staples
#                   (create once: xcrun notarytool store-credentials <name> --apple-id ... --team-id 4T73YMH6PG)
#   DMG=1           also produce dist/Rile-<version>.dmg
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME=Rile
BIN=rile
BUNDLE_ID=${BUNDLE_ID:-dev.rile.app}
TARGET=${TARGET:-}
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 1)
DIST=dist
APP="$DIST/$APP_NAME.app"

if [[ -z "${SIGN_IDENTITY:-}" ]]; then
  SIGN_IDENTITY=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:.*\)"/\1/p' | head -1)
  SIGN_IDENTITY=${SIGN_IDENTITY:--}
fi

echo "==> Building $BIN $VERSION ($BUILD)"
if [[ "$TARGET" == universal ]]; then
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target add "$t" >/dev/null
    cargo build --release -p rile-app --bin "$BIN" --target "$t"
  done
  mkdir -p target/universal/release
  lipo -create -output "target/universal/release/$BIN" \
    "target/aarch64-apple-darwin/release/$BIN" "target/x86_64-apple-darwin/release/$BIN"
  BIN_PATH="target/universal/release/$BIN"
elif [[ -n "$TARGET" ]]; then
  cargo build --release -p rile-app --bin "$BIN" --target "$TARGET"
  BIN_PATH="target/$TARGET/release/$BIN"
else
  cargo build --release -p rile-app --bin "$BIN"
  BIN_PATH="target/release/$BIN"
fi

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN_PATH" "$APP/Contents/MacOS/$BIN"
cp crates/rile-app/assets/Rile.icns "$APP/Contents/Resources/Rile.icns"
sed -e "s/__BUNDLE_ID__/$BUNDLE_ID/" -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" \
  packaging/macos/Info.plist > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

echo "==> Signing with: $SIGN_IDENTITY"
if [[ "$SIGN_IDENTITY" == "-" ]]; then
  codesign --force --deep --sign - "$APP"
else
  codesign --force --options runtime --timestamp \
    --entitlements packaging/macos/entitlements.plist \
    --sign "$SIGN_IDENTITY" "$APP"
fi
codesign --verify --strict --verbose=2 "$APP"

if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  echo "==> Notarizing"
  ZIP="$DIST/$APP_NAME-notarize.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  rm -f "$ZIP"
  spctl --assess --type execute --verbose "$APP"
fi

if [[ "${DMG:-0}" == 1 ]]; then
  DMG_PATH="$DIST/$APP_NAME-$VERSION.dmg"
  echo "==> Creating $DMG_PATH"
  STAGE=$(mktemp -d)
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$DMG_PATH"
  hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG_PATH" >/dev/null
  rm -rf "$STAGE"
  if [[ "$SIGN_IDENTITY" != "-" ]]; then
    codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG_PATH"
    if [[ -n "${NOTARY_PROFILE:-}" ]]; then
      xcrun notarytool submit "$DMG_PATH" --keychain-profile "$NOTARY_PROFILE" --wait
      xcrun stapler staple "$DMG_PATH"
    fi
  fi
fi

echo "==> Done: $APP"
