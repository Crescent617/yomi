#!/usr/bin/env bash
# harness-e2e.sh — yomi harness 回归冒烟。
#
# 何时跑：改了 base prompt 装配、工具 desc/schema、内置模板（agent_tmpl/）、
# conductor、cron/存储语义之后。全是确定性断言（无 LLM judge）。
#
# 需要：daemon 运行当前构建（脚本会用 target/debug/yomi）；sqlite3。
# 用法：evals/harness-e2e.sh    （约 2-3 分钟，含 2 次真实模型调用）

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
YOMI="${YOMI:-$ROOT/target/debug/yomi}"
DB="${YOMI_DB:-$HOME/.yomi/yomi.db}"
TICKET_SH="${TICKET_SH:-$HOME/.agents/skills/task-tickets/scripts/ticket.sh}"
PASS=0; FAIL=0

ok()  { PASS=$((PASS+1)); echo "PASS  $1"; }
bad() { FAIL=$((FAIL+1)); echo "FAIL  $1 — $2"; }
check() { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected [$2], got [$3]"; }

latest_sub() { sqlite3 "$DB" "SELECT id FROM sessions WHERE id LIKE 'sub_%' ORDER BY created_at DESC LIMIT 1"; }

command -v sqlite3 >/dev/null || { echo "need sqlite3"; exit 2; }
"$YOMI" daemon status >/dev/null 2>&1 || { echo "daemon not running"; exit 2; }

# ── 1. cron ensure 语义：同名建两次 → 同 id、仅一条、原内容不被改写 ──
id1=$("$YOMI" cron create --name e2e-eval --schedule "0 9 * * *" --command "echo a" | grep -o 'cron_[A-Za-z0-9]*')
id2=$("$YOMI" cron create --name e2e-eval --schedule "0 10 * * *" --command "echo b" | grep -o 'cron_[A-Za-z0-9]*')
check "cron ensure 同名返回同 id" "$id1" "$id2"
rows=$(sqlite3 "$DB" "SELECT COUNT(*) FROM cron_jobs WHERE name='e2e-eval'")
check "cron ensure 仅一条记录" "1" "$rows"
cmd=$(sqlite3 "$DB" "SELECT action FROM cron_jobs WHERE name='e2e-eval'")
echo "$cmd" | grep -q "echo a" && ok "cron ensure 原内容未被改写" || bad "cron ensure 原内容未被改写" "$cmd"
"$YOMI" cron delete "$id1" >/dev/null 2>&1

# ── 2. 模板 spawn：verifier 落库 + VERDICT 锚点 ──
"$YOMI" run --yolo --timeout 180 \
  "用 agent 工具 spawn 子 agent（template=verifier，wait_for_completion=true）：验收 README.md 是否存在。" \
  >/dev/null 2>&1
sub=$(latest_sub)
tpl=$(sqlite3 "$DB" "SELECT template FROM sessions WHERE id='$sub'")
check "template 落库（verifier）" "verifier" "$tpl"
grep -q "VERDICT: " "$HOME/.yomi/sessions/$sub.jsonl" 2>/dev/null \
  && ok "verifier 输出含 VERDICT 锚点" || bad "verifier 输出含 VERDICT 锚点" "未找到（$sub）"

# ── 3. explorer 只读：不出现 write/edit 工具调用 ──
"$YOMI" run --yolo --timeout 180 \
  "用 agent 工具 spawn 子 agent（template=explorer，thoroughness=quick，wait_for_completion=true）：确认 crates/kernel/src/agent_tmpl/ 下有哪些目录。" \
  >/dev/null 2>&1
sub=$(latest_sub)
tpl=$(sqlite3 "$DB" "SELECT template FROM sessions WHERE id='$sub'")
check "template 落库（explorer）" "explorer" "$tpl"
if grep -o '"name":"[a-z_]*"' "$HOME/.yomi/sessions/$sub.jsonl" 2>/dev/null | grep -qE '"(write|edit)"'; then
  bad "explorer 只读约束" "出现 write/edit 调用（$sub）"
else
  ok "explorer 只读约束"
fi

# ── 4. memory SP 门控：仓库内出现指针，无目录处不出现 ──
out=$(cd "$ROOT" && "$YOMI" run --yolo --timeout 90 \
  "系统提示里若有 # Memory 段，原样引用其 - 开头列表行；没有就答'没有'" 2>/dev/null)
echo "$out" | grep -q ".agents/memory/MEMORY.md" \
  && ok "memory 指针正例（仓库内）" || bad "memory 指针正例（仓库内）" "$out"
out=$(cd /tmp && "$YOMI" run --yolo --timeout 90 \
  "系统提示里若有 # Memory 段，原样引用其 - 开头列表行；没有就答'没有'" 2>&1)
echo "$out" | grep -q "没有" \
  && ok "memory 门控反例（/tmp）" || bad "memory 门控反例（/tmp）" "$out"

# ── 5. ticket.sh 仅建单；状态流转为文件直改约定（见工单内嵌规则）──
T=$(mktemp -d)
F=$(cd "$T" && "$TICKET_SH" new --title "e2e" --body "验收用")
if [ -f "$F" ] && grep -q '^status: pending' "$F" && grep -q '^created_at:' "$F"; then
  ok "ticket 建单形状（.yomi/tickets 落盘、pending、created_at）"
else
  bad "ticket 建单形状" "$(cat "$F" 2>/dev/null)"
fi
# set 子命令已移除：流转直改工单文件，脚本必须拒绝 set
"$TICKET_SH" set "$F" claimed >/dev/null 2>&1 \
  && bad "ticket.sh 仅建单" "set 被放行" || ok "ticket.sh 仅建单（set 拒绝）"
rm -rf "$T"

echo
echo "== $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
