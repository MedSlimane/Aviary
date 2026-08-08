#!/usr/bin/env bash
# Verifies the exact macOS application, updater archive, and DMG that CI is
# about to publish. This intentionally makes no network requests.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BUNDLE_ROOT="${1:-$PROJECT_DIR/src-tauri/target/universal-apple-darwin/release/bundle}"
MANIFEST="${2:-}"

fail() {
  echo "release verification failed: $*" >&2
  exit 1
}

single_match() {
  local search_root="$1"
  local kind="$2"
  local pattern="$3"
  local count
  local match

  [[ -d "$search_root" ]] || fail "missing bundle directory: $search_root"
  count="$(find "$search_root" -maxdepth 1 -type "$kind" -name "$pattern" | wc -l | tr -d ' ')"
  [[ "$count" == "1" ]] || fail "expected one $pattern in $search_root, found $count"
  match="$(find "$search_root" -maxdepth 1 -type "$kind" -name "$pattern" -print -quit)"
  printf '%s\n' "$match"
}

assert_universal() {
  local binary="$1"
  local architectures

  [[ -f "$binary" ]] || fail "missing executable: $binary"
  architectures="$(lipo -archs "$binary")"
  [[ " $architectures " == *" arm64 "* ]] || fail "$binary has no arm64 slice ($architectures)"
  [[ " $architectures " == *" x86_64 "* ]] || fail "$binary has no x86_64 slice ($architectures)"
}

assert_developer_id_signature() {
  local target="$1"
  local details

  codesign --verify --deep --strict --verbose=2 "$target"
  details="$(codesign -dvvv "$target" 2>&1)"
  printf '%s\n' "$details" | grep -q '^Authority=Developer ID Application:' \
    || fail "$target is not signed with Developer ID Application"
  printf '%s\n' "$details" | grep -Eq '^TeamIdentifier=[A-Z0-9]+' \
    || fail "$target has no Apple Team identifier"
  printf '%s\n' "$details" | grep -Eq 'flags=.*runtime' \
    || fail "$target does not enable hardened runtime"
  printf '%s\n' "$details" | grep -q '^Timestamp=' \
    || fail "$target has no secure signing timestamp"
}

assert_signed_container() {
  local target="$1"
  local details

  codesign --verify --strict --verbose=2 "$target"
  details="$(codesign -dvvv "$target" 2>&1)"
  printf '%s\n' "$details" | grep -q '^Authority=Developer ID Application:' \
    || fail "$target is not signed with Developer ID Application"
  printf '%s\n' "$details" | grep -Eq '^TeamIdentifier=[A-Z0-9]+' \
    || fail "$target has no Apple Team identifier"
  printf '%s\n' "$details" | grep -q '^Timestamp=' \
    || fail "$target has no secure signing timestamp"
}

APP="$(single_match "$BUNDLE_ROOT/macos" d '*.app')"
DMG="$(single_match "$BUNDLE_ROOT/dmg" f '*.dmg')"
UPDATE_ARCHIVE="$(single_match "$BUNDLE_ROOT/macos" f '*.app.tar.gz')"
UPDATE_SIGNATURE="$UPDATE_ARCHIVE.sig"

[[ -s "$UPDATE_ARCHIVE" ]] || fail "updater archive is empty: $UPDATE_ARCHIVE"
[[ -s "$UPDATE_SIGNATURE" ]] || fail "missing updater signature: $UPDATE_SIGNATURE"
command -v minisign >/dev/null 2>&1 || fail "minisign is required to verify the updater archive"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aviary-release.XXXXXX")"
MOUNT_DIR="$TEMP_ROOT/mount"
MOUNTED=0

cleanup() {
  if [[ "$MOUNTED" == "1" ]]; then
    hdiutil detach "$MOUNT_DIR" -quiet || true
  fi
  rm -rf "$TEMP_ROOT"
}
trap cleanup EXIT

UPDATER_PUBLIC_KEY="$TEMP_ROOT/aviary-updater.pub"
ENCODED_PUBLIC_KEY="$(plutil -extract plugins.updater.pubkey raw "$PROJECT_DIR/src-tauri/tauri.conf.json")"
printf '%s' "$ENCODED_PUBLIC_KEY" | openssl base64 -d -A > "$UPDATER_PUBLIC_KEY"
minisign -Vm "$UPDATE_ARCHIVE" -x "$UPDATE_SIGNATURE" -p "$UPDATER_PUBLIC_KEY"

assert_universal "$APP/Contents/MacOS/aviary"
assert_developer_id_signature "$APP"
for helper in aviary-media aviary-library aviary-launch; do
  assert_universal "$APP/Contents/MacOS/$helper"
  assert_developer_id_signature "$APP/Contents/MacOS/$helper"
done
xcrun stapler validate "$APP"

assert_signed_container "$DMG"
xcrun stapler validate "$DMG"
spctl --assess --type open --context context:primary-signature --verbose=2 "$DMG"

mkdir -p "$TEMP_ROOT/archive" "$MOUNT_DIR" "$TEMP_ROOT/quarantined"
tar -xzf "$UPDATE_ARCHIVE" -C "$TEMP_ROOT/archive"
ARCHIVED_APP="$(find "$TEMP_ROOT/archive" -type d -name '*.app' -print -quit)"
[[ -n "$ARCHIVED_APP" ]] || fail "updater archive contains no application bundle"
assert_universal "$ARCHIVED_APP/Contents/MacOS/aviary"
assert_developer_id_signature "$ARCHIVED_APP"
for helper in aviary-media aviary-library aviary-launch; do
  assert_universal "$ARCHIVED_APP/Contents/MacOS/$helper"
  assert_developer_id_signature "$ARCHIVED_APP/Contents/MacOS/$helper"
done
xcrun stapler validate "$ARCHIVED_APP"

hdiutil attach "$DMG" -readonly -nobrowse -mountpoint "$MOUNT_DIR" -quiet
MOUNTED=1
MOUNTED_APP="$(find "$MOUNT_DIR" -maxdepth 1 -type d -name '*.app' -print -quit)"
[[ -n "$MOUNTED_APP" ]] || fail "DMG contains no application bundle"
assert_universal "$MOUNTED_APP/Contents/MacOS/aviary"
assert_developer_id_signature "$MOUNTED_APP"
for helper in aviary-media aviary-library aviary-launch; do
  assert_universal "$MOUNTED_APP/Contents/MacOS/$helper"
  assert_developer_id_signature "$MOUNTED_APP/Contents/MacOS/$helper"
done

ditto "$MOUNTED_APP" "$TEMP_ROOT/quarantined/aviary.app"
xattr -w com.apple.quarantine "0081;0;AviaryReleaseCI;" "$TEMP_ROOT/quarantined/aviary.app"
spctl --assess --type execute --verbose=2 "$TEMP_ROOT/quarantined/aviary.app"

if command -v syspolicy_check >/dev/null 2>&1; then
  syspolicy_check distribution "$TEMP_ROOT/quarantined/aviary.app"
fi

if [[ -n "$MANIFEST" ]]; then
  [[ -s "$MANIFEST" ]] || fail "missing updater manifest: $MANIFEST"
  command -v jq >/dev/null 2>&1 || fail "jq is required to verify latest.json"

  CONFIG_VERSION="$(plutil -extract version raw "$PROJECT_DIR/src-tauri/tauri.conf.json")"
  MANIFEST_VERSION="$(jq -er '.version' "$MANIFEST")"
  [[ "$MANIFEST_VERSION" == "$CONFIG_VERSION" ]] \
    || fail "manifest version $MANIFEST_VERSION does not match app version $CONFIG_VERSION"

  ARM_URL="$(jq -er '.platforms["darwin-aarch64"].url' "$MANIFEST")"
  X64_URL="$(jq -er '.platforms["darwin-x86_64"].url' "$MANIFEST")"
  ARM_SIGNATURE="$(jq -er '.platforms["darwin-aarch64"].signature' "$MANIFEST")"
  X64_SIGNATURE="$(jq -er '.platforms["darwin-x86_64"].signature' "$MANIFEST")"

  [[ "$ARM_URL" == "$X64_URL" ]] \
    || fail "universal updater URLs differ between arm64 and x86_64"
  if [[ "$ARM_URL" != *.app.tar.gz \
    && ! "$ARM_URL" =~ ^https://api\.github\.com/repos/MedSlimane/Aviary/releases/assets/[0-9]+$ ]]; then
    fail "macOS updater URL is neither an app archive nor Aviary's GitHub asset API: $ARM_URL"
  fi

  ARM_SIGNATURE_FLAT="$(printf '%s' "$ARM_SIGNATURE" | tr -d '\r\n')"
  X64_SIGNATURE_FLAT="$(printf '%s' "$X64_SIGNATURE" | tr -d '\r\n')"
  LOCAL_SIGNATURE_FLAT="$(tr -d '\r\n' < "$UPDATE_SIGNATURE")"
  [[ "$ARM_SIGNATURE_FLAT" == "$X64_SIGNATURE_FLAT" ]] \
    || fail "universal updater signatures differ between arm64 and x86_64"
  [[ "$ARM_SIGNATURE_FLAT" == "$LOCAL_SIGNATURE_FLAT" ]] \
    || fail "latest.json signature does not match the generated updater signature"
fi

echo "Release verified: $APP"
echo "Updater verified: $UPDATE_ARCHIVE"
echo "Installer verified: $DMG"
