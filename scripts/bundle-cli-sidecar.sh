#!/usr/bin/env bash
# 构建 CLI 并摆成 tauri externalBin 约定的 sidecar：
#   crates/gui/binaries/yomi-<target-triple>[.exe]
#
# tauri build 按目标 triple 自动挑选该文件打进 app bundle（与主二进
# 制同目录落地）；GUI 启动时把所在目录 prepend 进 PATH（见
# crates/gui/src/main.rs 的 prepend_exe_dir_to_path），agent 的
# shell/hook/cron 子进程即可调用与 GUI 同版的 CLI。
#
# 用法: bundle-cli-sidecar.sh <target-triple> [profile=release] [--copy-only]
# 例:   bundle-cli-sidecar.sh aarch64-apple-darwin          # CI / 本机发布
#       bundle-cli-sidecar.sh aarch64-apple-darwin debug    # 本地 e2e（快）
#       bundle-cli-sidecar.sh aarch64-apple-darwin release --copy-only
#           # 只拷贝不编译：调用方已编好 CLI（如与 yomi-gui 单调用并
#           # 行叶 crate 的流水线），本脚本只负责摆位约定名。
#
# host triple 与目标一致时不加 --target——产物与常编共享 target/<profile>/
# 缓存（CI 的 Swatinem 缓存才有效）；交叉时才落 target/<triple>/<profile>/。
set -euo pipefail

TRIPLE="${1:?usage: bundle-cli-sidecar.sh <target-triple> [profile] [--copy-only]}"
PROFILE="${2:-release}"
[ "$PROFILE" = "debug" ] && PROFILE=dev # 口语别名
COPY_ONLY="${3:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOST="$(rustc -vV | awk '/^host:/{print $2}')"
# dev profile 的产物目录是 debug/，其余与 profile 同名。
OUT_DIR="$PROFILE"
[ "$PROFILE" = "dev" ] && OUT_DIR=debug
EXT=""
case "$TRIPLE" in
  *windows*) EXT=".exe" ;;
esac

if [ "$TRIPLE" = "$HOST" ]; then
  SRC="$ROOT/target/$OUT_DIR/yomi$EXT"
else
  SRC="$ROOT/target/$TRIPLE/$OUT_DIR/yomi$EXT"
fi

if [ "$COPY_ONLY" != "--copy-only" ]; then
  if [ "$TRIPLE" = "$HOST" ]; then
    cargo build -p cli --profile "$PROFILE"
  else
    cargo build -p cli --profile "$PROFILE" --target "$TRIPLE"
  fi
fi

DEST_DIR="$ROOT/crates/gui/binaries"
mkdir -p "$DEST_DIR"
cp "$SRC" "$DEST_DIR/yomi-$TRIPLE$EXT"
echo "sidecar ready: $DEST_DIR/yomi-$TRIPLE$EXT"
