#!/bin/bash
set -euo pipefail

# Homebrew Release Script for Yomi
# Usage: ./scripts/brew-release.sh [VERSION]
# If VERSION is not provided, reads from Cargo.toml

REPO="Crescent617/yomi"
TAP_REPO="Crescent617/homebrew-tap"
FORMULA_NAME="yomi"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log() {
    echo -e "${GREEN}[brew-release]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[brew-release]${NC} $1"
}

error() {
    echo -e "${RED}[brew-release]${NC} $1"
    exit 1
}

# Get version from argument or Cargo.toml
if [ $# -eq 1 ]; then
    VERSION="$1"
else
    VERSION=$(grep -E '^version\s*=\s*"[^"]+"' Cargo.toml | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
    if [ -z "$VERSION" ]; then
        error "Could not extract version from Cargo.toml"
    fi
fi

log "Releasing version: $VERSION"

# Check dependencies
for cmd in curl sha256sum git; do
    if ! command -v "$cmd" &> /dev/null; then
        error "$cmd is required but not installed"
    fi
done

# Calculate SHA256 hashes from GitHub release assets
# Format: platform -> sha256
declare -A SHA256S
PLATFORMS=(
    "aarch64-apple-darwin"
    "x86_64-unknown-linux-gnu"
)

log "Fetching release assets from GitHub..."

for platform in "${PLATFORMS[@]}"; do
    filename="yomi-${VERSION}-${platform}.tar.gz"
    url="https://github.com/${REPO}/releases/download/v${VERSION}/${filename}"

    log "Downloading ${filename}..."

    # Download and calculate sha256
    if ! sha256=$(curl -fsL "$url" | sha256sum | cut -d' ' -f1); then
        error "Failed to download or hash ${filename}"
    fi

    SHA256S[$platform]="$sha256"
    log "  SHA256: $sha256"
done

# Generate Homebrew formula
generate_formula() {
    cat <<EOF
class Yomi < Formula
  desc "AI coding assistant CLI featuring async agent loop and TUI interface"
  homepage "https://github.com/${REPO}"
  version "${VERSION}"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/${REPO}/releases/download/v${VERSION}/yomi-${VERSION}-aarch64-apple-darwin.tar.gz"
      sha256 "${SHA256S[aarch64-apple-darwin]}"
    end
  end

  on_linux do
    url "https://github.com/${REPO}/releases/download/v${VERSION}/yomi-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "${SHA256S[x86_64-unknown-linux-gnu]}"
  end

  def install
    bin.install "yomi"
  end

  test do
    system "#{bin}/yomi", "--version"
  end
end
EOF
}

# Clone or update homebrew-tap repo
TAP_DIR="/tmp/homebrew-tap-$$"

cleanup() {
    rm -rf "$TAP_DIR"
}
trap cleanup EXIT

log "Cloning ${TAP_REPO}..."
git clone --depth 1 "git@github.com:${TAP_REPO}.git" "$TAP_DIR" 2>/dev/null || \
    git clone --depth 1 "https://github.com/${TAP_REPO}.git" "$TAP_DIR"

FORMULA_PATH="${TAP_DIR}/Formula/${FORMULA_NAME}.rb"
mkdir -p "$(dirname "$FORMULA_PATH")"

log "Generating formula..."
generate_formula > "$FORMULA_PATH"

# Show diff
log "Formula changes:"
git --no-pager -C "$TAP_DIR" diff HEAD || true

# Commit and push
cd "$TAP_DIR"

if git diff --quiet HEAD; then
    warn "No changes to formula"
    exit 0
fi

git add -A
git commit -m "${FORMULA_NAME} ${VERSION}"

log "Pushing to ${TAP_REPO}..."
git push origin HEAD

log "Done! Formula updated to ${VERSION}"
log "Users can now run: brew update && brew upgrade ${FORMULA_NAME}"
