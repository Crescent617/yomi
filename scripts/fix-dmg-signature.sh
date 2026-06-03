#!/bin/bash
set -euo pipefail

# Fix Tauri-generated DMG ad-hoc signature issues.
# Must be run after `npx tauri build`.

DMG="target/release/bundle/dmg/Yomi_0.1.0_aarch64.dmg"

if [[ ! -f "$DMG" ]]; then
    echo "DMG not found: $DMG" >&2
    exit 1
fi

# Mount the generated DMG
echo "🔧 Fixing DMG signature..."
MOUNT_INFO=$(hdiutil attach "$DMG" -readonly -nobrowse 2>&1)
MOUNT=$(echo "$MOUNT_INFO" | grep -oE '/Volumes/[^ ]+' | tail -1)

TMP_APP="/tmp/Yomi-fix-$(date +%s).app"
cp -R "${MOUNT}/Yomi.app" "$TMP_APP"

hdiutil detach "$MOUNT" >/dev/null 2>&1

# Strip old broken signature and re-sign
codesign --remove-signature "$TMP_APP" 2>/dev/null || true
codesign --force --deep --sign - "$TMP_APP"

# Verify
codesign --verify --deep --strict "$TMP_APP"
echo "✅ Signature verified"

# Repackage DMG
rm -f "$DMG"
hdiutil create -volname "Yomi" -srcfolder "$TMP_APP" -ov -format UDZO "$DMG" >/dev/null

# Cleanup
rm -rf "$TMP_APP"

echo ""
echo "=== DMG ready ==="
echo "  Path: $DMG"
echo "  SHA256: $(shasum -a 256 "$DMG" | awk '{print $1}')"
