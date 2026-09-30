#!/bin/bash
set -euo pipefail

# Fix Tauri-generated DMG ad-hoc signature issues.
# Must be run after `npx tauri build`.

# Find repo root from script location
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

VERSION=$(grep -oE '"version":\s*"[^"]+"' "${ROOT_DIR}/crates/gui/tauri.conf.json" | head -1 | cut -d'"' -f4)
if [[ -z "$VERSION" ]]; then
    echo "Failed to read version from tauri.conf.json" >&2
    exit 1
fi

DMG="${ROOT_DIR}/target/release/bundle/dmg/Yomi_${VERSION}_aarch64.dmg"
if [[ ! -f "$DMG" ]]; then
    echo "DMG not found: $DMG" >&2
    exit 1
fi

# Create temp working dir
WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT

# Mount DMG
MOUNT=$(hdiutil attach "$DMG" -readonly -nobrowse 2>&1 | grep -oE '/Volumes/[^ ]+' | tail -1)
trap 'hdiutil detach "$MOUNT" >/dev/null 2>&1; rm -rf "$WORK_DIR"' EXIT

# Copy app to work dir
cp -R "${MOUNT}/Yomi.app" "$WORK_DIR/"

# Detach
hdiutil detach "$MOUNT" >/dev/null 2>&1
trap 'rm -rf "$WORK_DIR"' EXIT

# Re-sign with a stable identity so macOS TCC grants (accessibility /
# automation / ...) survive app upgrades. Preference order:
#   1. rcodesign + p12 (headless; no keychain/SecurityAgent involved)
#   2. apple codesign identity via SIGN_IDENTITY (needs unlocked keychain)
#   3. ad-hoc (new CDHash per build, grants reset)
SIGN_IDENTITY="${SIGN_IDENTITY:-}"
RCODESIGN="${RCODESIGN:-}"
SIGN_P12_PATH="${SIGN_P12_PATH:-}"
SIGN_P12_PASSWORD="${SIGN_P12_PASSWORD:-}"
codesign --remove-signature "$WORK_DIR/Yomi.app" 2>/dev/null || true
if [[ -n "$RCODESIGN" && -x "$RCODESIGN" && -n "$SIGN_P12_PATH" ]]; then
    echo "Signing with rcodesign (p12: $SIGN_P12_PATH)"
    "$RCODESIGN" sign --p12-file "$SIGN_P12_PATH" --p12-password "$SIGN_P12_PASSWORD" "$WORK_DIR/Yomi.app"
elif [[ -n "$SIGN_IDENTITY" ]] && security find-identity -p codesigning | grep -q "\"$SIGN_IDENTITY\""; then
    echo "Signing with identity: $SIGN_IDENTITY"
    codesign --force --deep --sign "$SIGN_IDENTITY" "$WORK_DIR/Yomi.app"
else
    echo "Signing ad-hoc (no rcodesign p12, SIGN_IDENTITY='${SIGN_IDENTITY:-unset}')"
    codesign --force --deep --sign - "$WORK_DIR/Yomi.app"
fi
codesign --verify --deep --strict "$WORK_DIR/Yomi.app"
echo "✅ Signature verified"

# Repackage DMG
rm -f "$DMG"
hdiutil create -volname "Yomi" -srcfolder "$WORK_DIR" -ov -format UDZO "$DMG" >/dev/null

echo ""
echo "=== DMG ready ==="
echo "  Path: $DMG"
echo "  SHA256: $(shasum -a 256 "$DMG" | awk '{print $1}')"
