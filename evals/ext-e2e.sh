#!/usr/bin/env bash
# ext-e2e.sh — 扩展包系统隔离端到端测试。
#
# 隔离方式：独立 data_dir（独立 config）+ YOMI_SOCKET 指向独立 socket，
# 与日常 daemon 完全并存。装一个带全资源（cron/hooks/bin/snippets）的
# demo 包，逐项断言 install/list/remove 全链（CLI→wire→kernel→fs/sqlite），
# 并用 cron shell job 验证 PATH 注入（不依赖模型调用）。注册表是
# extensions/ext.lock 单文件注册表（Cargo.lock 式 [[extensions]]；
# ext.toml 归作者原封不动，不再有 sqlite ext_installs 表）。lock 在
# 包目录外：作者随包自带 ext.lock 伪造不了所有权。
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
# 等待上限放宽到 120s：macOS 对全新 debug 二进制逐个做安全评估
# （syspolicyd，实测每文件数百毫秒且系统级串行），刚 cargo build 完
# 的首跑可能远超旧的 30s 上限（2026-10-01 实测偶发失败、重跑即过）。
for _ in $(seq 1 120); do
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
# 作者随包自带 ext.lock：必须被当普通包文件复制（不构成所有权证明，
# 真正的注册表是包外的单文件 ext.lock）。
echo "author's own lock, not ownership proof" > "$PKG/ext.lock"

# ── 2. install：报告 + 文件系统 + sqlite 三面对账 ──
out=$("$YOMI" extension install "$PKG" 2>&1) || { echo "install failed: $out"; exit 2; }
echo "$out" | grep -q "Installed extension e2e-demo" && ok "install report" || bad "install report" "$out"
[ -d "$DATA/extensions/e2e-demo" ] && [ ! -L "$DATA/extensions/e2e-demo" ] && ok "extensions copy" || bad "extensions copy" "not a real dir"
[ -f "$DATA/extensions/e2e-demo/bin/e2e-recall" ] && ok "bin copied into ext dir" || bad "bin copied into ext dir" "missing"
[ -L "$DATA/hooks/pre_tool_use/90-e2e-guard" ] && ok "hook mounted" || bad "hook mounted" "missing"
[ -L "$DATA/bin/e2e-recall" ] && ok "bin mounted" || bad "bin mounted" "missing"
rows=$(sqlite3 "$DB" "SELECT COUNT(*) FROM cron_jobs WHERE name LIKE 'ext:e2e-demo:%'")
check "cron adopted (2 jobs)" "2" "$rows"
content=$(sqlite3 "$DB" "SELECT action FROM cron_jobs WHERE name='ext:e2e-demo:tick'")
echo "$content" | grep -q "tick via file" && ok "cron message from file" || bad "cron message from file" "$content"
# 单文件注册表 ext.lock：来源、hash、资源清单都落盘；ext.toml 是
# 作者的 manifest，必须原封不动。
lock="$DATA/extensions/ext.lock"
grep -q 'name = "e2e-demo"' "$lock" && ok "registry entry present" || bad "registry entry present" "missing"
grep -q '^source = ' "$lock" && ok "provenance in registry" || bad "provenance in registry" "missing"
grep -q '^content_hash = "[0-9a-f]\{64\}"' "$lock" && ok "content hash in registry" || bad "content hash in registry" "missing"
cmp -s "$PKG/ext.toml" "$DATA/extensions/e2e-demo/ext.toml" && ok "ext.toml verbatim" || bad "ext.toml verbatim" "changed"
# 作者随包自带的 ext.lock 只是普通包文件：随复制进目录、不构成所有权。
[ -f "$DATA/extensions/e2e-demo/ext.lock" ] && ok "authored lock copied as plain file" || bad "authored lock copied as plain file" "missing"
[ -f "$lock" ] && ok "registry lock outside package dir" || bad "registry lock outside package dir" "missing"

# 幂等重跑
out=$("$YOMI" extension install "$PKG" 2>&1)
echo "$out" | grep -q "exists, untouched" && ok "reinstall idempotent" || bad "reinstall idempotent" "$out"
rows=$(sqlite3 "$DB" "SELECT COUNT(*) FROM cron_jobs WHERE name LIKE 'ext:e2e-demo:%'")
check "reinstall no duplicate cron" "2" "$rows"

# 升级：包内容变化后重装 = 原位更新——ext.lock 换 content_hash、
# 安装目录内容刷新、ext.toml 仍逐字、cron 不重复。
old_hash=$(grep '^content_hash = ' "$lock")
sed -i '' 's/^version = "0.1.0"$/version = "0.2.0"/' "$PKG/ext.toml"
echo "e2e convention: always pass, now v2" > "$PKG/snippets/convention.md"
# v2 同时撤掉 hook：原位替换后旧版本的 hook 挂载必然悬空——必须被
# 清扫（否则 phantom-block 别的扩展装同名槽位，health 还看不见）。
rm -f "$PKG/hooks/pre_tool_use/90-e2e-guard"
out=$("$YOMI" extension install "$PKG" 2>&1)
echo "$out" | grep -q "Installed extension e2e-demo v0.2.0" && ok "upgrade report" || bad "upgrade report" "$out"
new_hash=$(grep '^content_hash = ' "$lock")
[ "$old_hash" != "$new_hash" ] && ok "upgrade bumps content hash" || bad "upgrade bumps content hash" "$lock"
grep -q "now v2" "$DATA/extensions/e2e-demo/snippets/convention.md" && ok "upgrade refreshes content" || bad "upgrade refreshes content" "stale"
[ ! -e "$DATA/hooks/pre_tool_use/90-e2e-guard" ] && ok "upgrade sweeps stale mount" || bad "upgrade sweeps stale mount" "dangling hook symlink"
cmp -s "$PKG/ext.toml" "$DATA/extensions/e2e-demo/ext.toml" && ok "upgrade ext.toml verbatim" || bad "upgrade ext.toml verbatim" "changed"
rows=$(sqlite3 "$DB" "SELECT COUNT(*) FROM cron_jobs WHERE name LIKE 'ext:e2e-demo:%'")
check "upgrade no duplicate cron" "2" "$rows"

# ── 3. list：health 必须是 ok（hash 比对真正生效，不是子串误配）──
out=$("$YOMI" extension list)
health=$(echo "$out" | awk '$1 == "e2e-demo" {print $3}')
check "extension list health ok" "ok" "$health"

# health 异常态逐一诱发 → 断言 → 恢复（modified/oversized/unreadable/foreign）
echo "tampered" >> "$DATA/extensions/e2e-demo/snippets/convention.md"
health=$("$YOMI" extension list | awk '$1 == "e2e-demo" {print $3}')
check "health modified" "modified" "$health"
printf 'e2e convention: always pass, now v2\n' > "$DATA/extensions/e2e-demo/snippets/convention.md"
health=$("$YOMI" extension list | awk '$1 == "e2e-demo" {print $3}')
check "health back to ok (modified)" "ok" "$health"

head -c 2097152 /dev/zero > "$DATA/extensions/e2e-demo/big.bin"
health=$("$YOMI" extension list | awk '$1 == "e2e-demo" {print $3}')
check "health oversized" "oversized" "$health"
rm -f "$DATA/extensions/e2e-demo/big.bin"
health=$("$YOMI" extension list | awk '$1 == "e2e-demo" {print $3}')
check "health back to ok (oversized)" "ok" "$health"

chmod 000 "$DATA/extensions/e2e-demo/bin/e2e-recall"
health=$("$YOMI" extension list | awk '$1 == "e2e-demo" {print $3}')
check "health unreadable" "unreadable" "$health"
chmod 755 "$DATA/extensions/e2e-demo/bin/e2e-recall"
health=$("$YOMI" extension list | awk '$1 == "e2e-demo" {print $3}')
check "health back to ok (unreadable)" "ok" "$health"

mkdir -p "$DATA/extensions/hand-drop"
printf '[ext]\nname = "hand-drop"\nversion = "0"\ndescription = "user dropped"\n' > "$DATA/extensions/hand-drop/ext.toml"
health=$("$YOMI" extension list | awk '$1 == "hand-drop" {print $3}')
check "health foreign" "foreign" "$health"
rm -rf "$DATA/extensions/hand-drop"

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
! grep -q 'e2e-demo' "$lock" && ok "registry entry removed with remove" || bad "registry entry removed with remove" "still there"

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
