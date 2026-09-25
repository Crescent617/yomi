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

  # GUI 的 agent 功能依赖 PATH 上的 yomi CLI（doc/session wait/cron
  # 等子命令）——装 cask 时把 formula 一并装上，PATH 由 brew 链接
  # 自动就绪。注意：依赖只在 install 时保证存在；升级 cask 不会连
  # 带升级 formula（brew 语义），formula 随 brew upgrade 全量或单
  # 独升级。
  depends_on formula: "yomi"

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
