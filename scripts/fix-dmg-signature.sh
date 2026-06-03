#!/bin/bash
set -euo pipefail

# Fix Tauri-generated DMG ad-hoc signature issues.
# Must be run after `npx tauri build`.

DMG="target/release/bundle/dmg/Yomi_0.1.1_aarch64.dmg"

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

# Re-sign
codesign --remove-signature "$WORK_DIR/Yomi.app" 2>/dev/null || true
codesign --force --deep --sign - "$WORK_DIR/Yomi.app"
codesign --verify --deep --strict "$WORK_DIR/Yomi.app"
echo "✅ Signature verified"

# Repackage DMG
rm -f "$DMG"
hdiutil create -volname "Yomi" -srcfolder "$WORK_DIR" -ov -format UDZO "$DMG" >/dev/null

echo ""
echo "=== DMG ready ==="
echo "  Path: $DMG"
echo "  SHA256: $(shasum -a 256 "$DMG" | awk '{print $1}')"
