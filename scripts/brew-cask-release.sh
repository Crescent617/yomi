#!/usr/bin/env bash
set -euo pipefail

# Homebrew Cask Release Script for Yomi GUI
# Usage: ./scripts/brew-cask-release.sh [VERSION]
# If VERSION is not provided, reads from crates/gui/tauri.conf.json
#
# This script:
# 1. Runs `just gui-build` to build and sign the DMG locally
# 2. Creates/uploads the DMG to the GitHub release
# 3. Updates the Homebrew Cask with the new SHA256
# 4. Pushes the updated Cask to homebrew-tap

REPO="Crescent617/yomi"
TAP_REPO="Crescent617/homebrew-tap"
CASK_NAME="yomi-app"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log() {
    echo -e "${GREEN}[brew-cask-release]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[brew-cask-release]${NC} $1"
}

error() {
    echo -e "${RED}[brew-cask-release]${NC} $1"
    exit 1
}

# Get version from argument or tauri.conf.json
if [ $# -eq 1 ]; then
    VERSION="$1"
else
    VERSION=$(grep -oE '"version":\s*"[^"]+"' crates/gui/tauri.conf.json | head -1 | cut -d'"' -f4)
    if [ -z "$VERSION" ]; then
        error "Could not extract version from crates/gui/tauri.conf.json"
    fi
fi

log "Releasing GUI version: $VERSION"

# Check dependencies
for cmd in just gh shasum git; do
    if ! command -v "$cmd" &> /dev/null; then
        error "$cmd is required but not installed"
    fi
done

DMG_PATH="target/release/bundle/dmg/Yomi_${VERSION}_aarch64.dmg"
DMG_FILENAME="Yomi_${VERSION}_aarch64.dmg"

# Step 1: Build the DMG
if [ -f "$DMG_PATH" ]; then
    warn "DMG already exists at $DMG_PATH. Rebuild? (y/N)"
    read -r response
    if [[ "$response" =~ ^[Yy]$ ]]; then
        log "Building DMG with just gui-build..."
        just gui-build
    else
        log "Using existing DMG."
    fi
else
    log "Building DMG with just gui-build..."
    just gui-build
fi

if [ ! -f "$DMG_PATH" ]; then
    error "DMG not found at $DMG_PATH after build"
fi

# Calculate SHA256
DMG_SHA256=$(shasum -a 256 "$DMG_PATH" | cut -d' ' -f1)
log "DMG SHA256: $DMG_SHA256"

# Step 2: Upload to GitHub release
log "Checking GitHub release v${VERSION}..."
if ! gh release view "v${VERSION}" --repo "$REPO" &> /dev/null; then
    warn "Release v${VERSION} does not exist. Creating..."
    gh release create "v${VERSION}" \
        --repo "$REPO" \
        --title "Yomi v${VERSION}" \
        --notes "GUI release for macOS (Apple Silicon). Install via: brew install --cask ${TAP_REPO}/${CASK_NAME}"
fi

log "Uploading DMG to GitHub release v${VERSION}..."
gh release upload "v${VERSION}" \
    --repo "$REPO" \
    "$DMG_PATH" \
    --clobber

log "DMG uploaded successfully"

# Step 3: Update Homebrew Cask
TAP_DIR="/tmp/homebrew-tap-$$"

cleanup() {
    rm -rf "$TAP_DIR"
}
trap cleanup EXIT

log "Cloning ${TAP_REPO}..."
git clone --depth 1 "https://github.com/${TAP_REPO}.git" "$TAP_DIR" 2>/dev/null || \
    git clone --depth 1 "git@github.com:${TAP_REPO}.git" "$TAP_DIR" 2>/dev/null

CASK_PATH="${TAP_DIR}/Casks/${CASK_NAME}.rb"

# Generate Cask
cat > "$CASK_PATH" <<EOF
cask "${CASK_NAME}" do
  version "${VERSION}"
  sha256 "${DMG_SHA256}"

  url "https://github.com/${REPO}/releases/download/v#{version}/Yomi_#{version}_aarch64.dmg"
  name "Yomi"
  desc "AI coding assistant with GUI"
  homepage "https://github.com/${REPO}"

  app "Yomi.app"

  zap trash: [
    "~/.yomi",
    "~/Library/Logs/yomi",
  ]
end
EOF

log "Cask updated:"
cat "$CASK_PATH"

# Step 4: Commit and push
cd "$TAP_DIR"

git add -A
git commit -m "Bump ${CASK_NAME} to v${VERSION}"

log "Pushing to ${TAP_REPO}..."
git push origin HEAD

log "Done! Cask updated to ${VERSION}"
log "Users can now run: brew update && brew upgrade --cask ${TAP_REPO}/${CASK_NAME}"
