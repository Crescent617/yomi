#!/usr/bin/env bash
# 从 CHANGELOG.md 抽取指定版本的 section 作为 release notes。
# Usage: extract-release-notes.sh <VERSION> <output-file>
# create-release（初始 notes）与 audit（重抽干净 notes + 缺失段）
# 共用此脚本，避免两份 awk 漂移（2026-09-12 评审）。
set -euo pipefail

VERSION="$1"
OUT="$2"

awk -v ver="$VERSION" '
  /^## \[/ {
    if (found) exit
    if (index($0, "## [" ver "]") == 1) found=1
    next
  }
  found && !started && /^[[:space:]]*$/ { next }
  found { started=1; out = out $0 "\n" }
  END { sub(/\n+$/, "", out); if (out != "") print out }
' CHANGELOG.md > "$OUT"

if [ ! -s "$OUT" ]; then
  echo "::warning::CHANGELOG.md 中未找到 [${VERSION}] 的条目，Release 将没有 release note"
fi
cat "$OUT"
