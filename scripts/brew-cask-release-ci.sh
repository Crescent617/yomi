#!/usr/bin/env bash
set -euo pipefail

# Homebrew Cask Release Script for Yomi GUI (CI version)
# Usage: ./scripts/brew-cask-release-ci.sh <VERSION> <DMG_SHA256>
#
# This script only generates and pushes the cask — assumes DMG is already on GitHub release.

REPO="Crescent617/yomi"
TAP_REPO="Crescent617/homebrew-tap"
CASK_NAME="yomi-app"

VERSION="$1"
DMG_SHA256="$2"

GREEN='\033[0;32m'
NC='\033[0m'
log() { echo -e "${GREEN}[brew-cask-ci]${NC} $1"; }

if [ -n "${HOMEBREW_TAP_TOKEN:-}" ]; then
    TAP_URL="https://x-access-token:${HOMEBREW_TAP_TOKEN}@github.com/${TAP_REPO}.git"
else
    TAP_URL="https://github.com/${TAP_REPO}.git"
fi

TAP_DIR="/tmp/homebrew-tap-$$"
cleanup() {
    rm -rf "$TAP_DIR"
}
trap cleanup EXIT

log "Cloning ${TAP_REPO}..."
git clone --depth 1 "$TAP_URL" "$TAP_DIR"

CASK_PATH="${TAP_DIR}/Casks/${CASK_NAME}.rb"
mkdir -p "$(dirname "$CASK_PATH")"

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

cd "$TAP_DIR"

if git diff --quiet HEAD; then
    log "No changes to cask"
    exit 0
fi

git add -A
git commit -m "Bump ${CASK_NAME} to v${VERSION}"

git push origin HEAD
log "Done! Cask updated to ${VERSION}"
