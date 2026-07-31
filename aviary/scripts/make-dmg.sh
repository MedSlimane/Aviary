#!/usr/bin/env bash
#
# Builds the distributable DMG with `create-dmg` (brew install create-dmg).
#
# Tauri's own bundler already emits a DMG, but it produces a plain disk image
# with no layout. This gives the window a real icon arrangement and an
# /Applications drop target, which is what people expect on macOS.
#
# Usage: ./scripts/make-dmg.sh [path/to/aviary.app]

set -euo pipefail

cd "$(dirname "$0")/.."

APP="${1:-src-tauri/target/universal-apple-darwin/release/bundle/macos/aviary.app}"
VERSION="$(python3 -c 'import json;print(json.load(open("src-tauri/tauri.conf.json"))["version"])')"
# `dist/` belongs to Vite; release artifacts get their own directory.
OUT_DIR="release"
DMG="$OUT_DIR/Aviary_${VERSION}_universal.dmg"

if [[ ! -d "$APP" ]]; then
  echo "No app bundle at $APP" >&2
  echo "Run: bun run tauri build --target universal-apple-darwin" >&2
  exit 1
fi

# `lipo` strips the linker's ad-hoc signature, so a universal bundle arrives
# completely unsigned — macOS then reports it as damaged rather than merely
# untrusted. Re-sign ad-hoc so the bundle has a valid (if unauthenticated)
# signature. This is not notarisation and does not satisfy Gatekeeper; it only
# ensures the app is well-formed. Set SIGN_IDENTITY to use a real Developer ID.
IDENTITY="${SIGN_IDENTITY:--}"
echo "Signing $APP with identity: $IDENTITY"
codesign --force --deep --timestamp=none --sign "$IDENTITY" "$APP"
codesign --verify --deep --strict "$APP"

mkdir -p "$OUT_DIR"
rm -f "$DMG"

# create-dmg returns 2 when it cannot set a custom volume icon, which happens
# on machines without an icon utility. The image itself is still valid, so that
# case is tolerated while real failures are not.
set +e
create-dmg \
  --volname "Aviary $VERSION" \
  --window-pos 200 120 \
  --window-size 660 420 \
  --icon-size 120 \
  --icon "aviary.app" 165 190 \
  --hide-extension "aviary.app" \
  --app-drop-link 495 190 \
  --no-internet-enable \
  "$DMG" \
  "$APP"
STATUS=$?
set -e

if [[ $STATUS -ne 0 && $STATUS -ne 2 ]]; then
  echo "create-dmg failed with status $STATUS" >&2
  exit $STATUS
fi

[[ -f "$DMG" ]] || { echo "create-dmg produced no image" >&2; exit 1; }

echo
echo "Built $DMG"
shasum -a 256 "$DMG"
