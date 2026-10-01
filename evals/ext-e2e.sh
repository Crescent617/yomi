#!/usr/bin/env bash
# ext-e2e.sh — 扩展包系统隔离端到端测试。
#
# 隔离方式：独立 data_dir（独立 config）+ YOMI_SOCKET 指向独立 socket，
# 与日常 daemon 完全并存。装一个带全资源（cron/hooks/bin/snippets）的
# demo 包，逐项断言 install/list/remove 全链（CLI→wire→kernel→fs/sqlite），
# 并用 cron shell job 验证 PATH 注入（不依赖模型调用）。
#
# 用法：evals/ext-e2e.sh    （需 target/debug/yomi；约 1 分钟，无模型调用）

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
YOMI="${YOMI:-$ROOT/target/debug/yomi}"
E2E="$(mktemp -d /tmp/yomi-ext-e2e.XXXXXX)"
DATA="$E2E/data"
SOCK="$E2E/daemon.sock"
DB="$DATA/yomi.db"
PKG="$E2E/ext-demo"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "PASS  $1"; }
bad() { FAIL=$((FAIL+1)); echo "FAIL  $1 — $2"; }
check() { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected [$2], got [$3]"; }

command -v sqlite3 >/dev/null || { echo "need sqlite3"; exit 2; }
[ -x "$YOMI" ] || { echo "build first: cargo build -p cli"; exit 2; }

cleanup() {
  YOMI_SOCKET="unix://$SOCK" "$YOMI" daemon stop >/dev/null 2>&1
  rm -rf "$E2E"
}
trap cleanup EXIT

# ── 0. 隔离环境：data_dir + 独立 socket ──
# 注意：从 yomi 会话里跑本脚本时，进程环境自带 YOMI_DATA_DIR（agent 子
# 进程注入）——必须显式改指临时目录，否则 env override 盖过 config。
# pid 文件/socket 也随之隔离。
mkdir -p "$DATA"
export YOMI_DATA_DIR="$DATA"
export YOMI_SOCKET="unix://$SOCK"

nohup "$YOMI" daemon start >"$E2E/daemon.log" 2>&1 &
for _ in $(seq 1 30); do
  YOMI_SOCKET="unix://$SOCK" "$YOMI" daemon status >/dev/null 2>&1 && break
  sleep 1
done
YOMI_SOCKET="unix://$SOCK" "$YOMI" daemon status >/dev/null 2>&1 || { echo "isolated daemon failed to start"; cat "$E2E/daemon.log"; exit 2; }
ok "isolated daemon up"

# ── 1. 造 demo 包：全资源（manifest + cron×2 + hook + bin + snippet）──
mkdir -p "$PKG/prompts" "$PKG/hooks/pre_tool_use" "$PKG/bin" "$PKG/snippets"
cat > "$PKG/ext.toml" <<'TOML'
[ext]
name = "e2e-demo"
version = "0.1.0"
description = "e2e test package"

[[cron]]
name = "tick"
schedule = "0 9 * * *"
message_file = "prompts/tick.txt"

[[cron]]
name = "tock"
schedule = "0 10 * * *"
message = "tock inline"
TOML
echo "tick via file" > "$PKG/prompts/tick.txt"
printf '#!/bin/sh\nexit 0\n' > "$PKG/hooks/pre_tool_use/90-e2e-guard"
chmod +x "$PKG/hooks/pre_tool_use/90-e2e-guard"
printf '#!/bin/sh\necho e2e-recall-ok\n' > "$PKG/bin/e2e-recall"
chmod +x "$PKG/bin/e2e-recall"
echo "e2e convention: always pass" > "$PKG/snippets/convention.md"

# ── 2. install：报告 + 文件系统 + sqlite 三面对账 ──
out=$("$YOMI" extension install "$PKG" 2>&1) || { echo "install failed: $out"; exit 2; }
echo "$out" | grep -q "Installed extension e2e-demo" && ok "install report" || bad "install report" "$out"
[ -L "$DATA/extensions/e2e-demo" ] && ok "extensions symlink" || bad "extensions symlink" "not a symlink"
[ -L "$DATA/hooks/pre_tool_use/90-e2e-guard" ] && ok "hook mounted" || bad "hook mounted" "missing"
[ -L "$DATA/bin/e2e-recall" ] && ok "bin mounted" || bad "bin mounted" "missing"
rows=$(sqlite3 "$DB" "SELECT COUNT(*) FROM cron_jobs WHERE name LIKE 'ext:e2e-demo:%'")
check "cron adopted (2 jobs)" "2" "$rows"
content=$(sqlite3 "$DB" "SELECT action FROM cron_jobs WHERE name='ext:e2e-demo:tick'")
echo "$content" | grep -q "tick via file" && ok "cron message from file" || bad "cron message from file" "$content"
rec=$(sqlite3 "$DB" "SELECT COUNT(*) FROM ext_installs WHERE name='e2e-demo' AND mode='symlink'")
check "install record written" "1" "$rec"

# 幂等重跑
out=$("$YOMI" extension install "$PKG" 2>&1)
echo "$out" | grep -q "exists, untouched" && ok "reinstall idempotent" || bad "reinstall idempotent" "$out"
rows=$(sqlite3 "$DB" "SELECT COUNT(*) FROM cron_jobs WHERE name LIKE 'ext:e2e-demo:%'")
check "reinstall no duplicate cron" "2" "$rows"

# ── 3. list：记录 + health ──
out=$("$YOMI" extension list)
echo "$out" | grep -q "e2e-demo" && echo "$out" | grep -q "ok" && ok "extension list" || bad "extension list" "$out"

# ── 4. PATH 注入：cron shell job 触发，子进程找 bin 命令 ──
"$YOMI" cron create --name e2e-pathprobe --schedule "0 0 1 1 *" --command "e2e-recall > $E2E/path-probe.txt" >/dev/null
jid=$(sqlite3 "$DB" "SELECT id FROM cron_jobs WHERE name='e2e-pathprobe'")
"$YOMI" cron trigger "$jid" >/dev/null 2>&1
sleep 2
probe=$(cat "$E2E/path-probe.txt" 2>/dev/null)
check "bin on PATH in yomi children" "e2e-recall-ok" "$probe"
"$YOMI" cron delete "$jid" >/dev/null

# ── 5. remove：全链回滚 ──
out=$("$YOMI" extension remove e2e-demo 2>&1) || { echo "remove failed: $out"; exit 2; }
echo "$out" | grep -q "Removed extension e2e-demo" && ok "remove report" || bad "remove report" "$out"
[ ! -e "$DATA/hooks/pre_tool_use/90-e2e-guard" ] && ok "hook unmounted" || bad "hook unmounted" "still there"
[ ! -e "$DATA/bin/e2e-recall" ] && ok "bin unmounted" || bad "bin unmounted" "still there"
[ ! -e "$DATA/extensions/e2e-demo" ] && ok "extensions dir removed" || bad "extensions dir removed" "still there"
rows=$(sqlite3 "$DB" "SELECT COUNT(*) FROM cron_jobs WHERE name LIKE 'ext:e2e-demo:%'")
check "cron swept" "0" "$rows"
rec=$(sqlite3 "$DB" "SELECT COUNT(*) FROM ext_installs WHERE name='e2e-demo'")
check "record deleted" "0" "$rec"

# 槽位保护：用户文件占 bin 槽位时 install 拒绝且不覆盖
mkdir -p "$DATA/bin"
echo "user-owned" > "$DATA/bin/e2e-recall"
if "$YOMI" extension install "$PKG" >/dev/null 2>&1; then
  bad "conflict refusal" "install succeeded over user file"
else
  ok "conflict refusal"
fi
check "user file untouched" "user-owned" "$(cat "$DATA/bin/e2e-recall")"
rm -f "$DATA/bin/e2e-recall"

echo
echo "ext-e2e: $PASS passed, $FAIL failed"
[ "$FAIL" = "0" ]
