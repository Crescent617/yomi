#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
FLAKE="$ROOT_DIR/flake.nix"
PKG_LOCK="$ROOT_DIR/crates/gui/frontend/package-lock.json"

QUIET=0

usage() {
    echo "Usage: $0 [--quiet]"
    echo "  --quiet    Suppress non-error output"
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --quiet)
            QUIET=1
            shift
            ;;
        -h|--help)
            usage
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage
            ;;
    esac
done

log() {
    if [[ $QUIET -eq 0 ]]; then
        echo "$@"
    fi
}

if [[ ! -f "$FLAKE" ]]; then
    echo "Error: $FLAKE not found" >&2
    exit 1
fi

if [[ ! -f "$PKG_LOCK" ]]; then
    echo "Error: $PKG_LOCK not found" >&2
    exit 1
fi

log "==> Computing npm deps hash for $PKG_LOCK ..."
NEW_HASH=$(nix run nixpkgs#prefetch-npm-deps -- "$PKG_LOCK")
log "    New hash: $NEW_HASH"

OLD_HASH=$(grep -oP 'npmDepsHash = "\K[^"]+' "$FLAKE" || true)
if [[ -n "$OLD_HASH" ]]; then
    log "    Old hash: $OLD_HASH"
fi

sed -i "s|npmDepsHash = \"[^\"]*\";|npmDepsHash = \"$NEW_HASH\";|" "$FLAKE"

log "==> Done. flake.nix updated."
log "    Verify with: nix build .#yomi-gui"
