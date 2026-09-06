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
# sessions 落盘在 data_dir（即 db 所在目录）的 sessions/ 下——隔离测试
# （YOMI_DB 指向别处）时跟着走，不写死 ~/.yomi。
SESS_DIR="$(dirname "$DB")/sessions"
TICKET_SH="${KB_PY:-$HOME/.agents/skills/kanban/scripts/kb.py}"
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

# ── 1.5 cron precheck 闸门：create 落库、update 清除（调度路径的门控由
# kernel 单测覆盖，这里只验 CLI→wire→DB 的管道）──
gid=$("$YOMI" cron create --name e2e-gate --schedule "0 9 * * *" --command "echo a" --precheck "test -f /tmp/x" | grep -o 'cron_[A-Za-z0-9]*')
pre=$(sqlite3 "$DB" "SELECT precheck FROM cron_jobs WHERE id='$gid'")
check "cron precheck create 落库" "test -f /tmp/x" "$pre"
"$YOMI" cron update "$gid" --precheck "exit 0" >/dev/null 2>&1
pre=$(sqlite3 "$DB" "SELECT precheck FROM cron_jobs WHERE id='$gid'")
check "cron precheck update 设置" "exit 0" "$pre"
"$YOMI" cron update "$gid" --precheck "" >/dev/null 2>&1
pre=$(sqlite3 "$DB" "SELECT precheck IS NULL FROM cron_jobs WHERE id='$gid'")
check "cron precheck update 空串清除" "1" "$pre"
"$YOMI" cron delete "$gid" >/dev/null 2>&1

# ── 2. 模板 spawn：verifier 落库 + VERDICT 锚点 ──
"$YOMI" run --yolo --timeout 180 \
  "用 agent 工具 spawn 子 agent（template=verifier，wait_for_completion=true）：验收 README.md 是否存在。" \
  >/dev/null 2>&1
sub=$(latest_sub)
tpl=$(sqlite3 "$DB" "SELECT template FROM sessions WHERE id='$sub'")
check "template 落库（verifier）" "verifier" "$tpl"
# 「（${sub}）」的大括号不可省：UTF-8 locale 下全角括号会被并入变量名，
# 触发 set -u 的 unbound variable 直接 abort（失败分支才炸，极隐蔽）。
grep -q "VERDICT: " "$SESS_DIR/$sub.jsonl" 2>/dev/null \
  && ok "verifier 输出含 VERDICT 锚点" || bad "verifier 输出含 VERDICT 锚点" "未找到（${sub}）"

# ── 3. explorer 只读：不出现 write/edit 工具调用 ──
"$YOMI" run --yolo --timeout 180 \
  "用 agent 工具 spawn 子 agent（template=explorer，thoroughness=quick，wait_for_completion=true）：确认 crates/kernel/src/agent_tmpl/ 下有哪些目录。" \
  >/dev/null 2>&1
sub=$(latest_sub)
tpl=$(sqlite3 "$DB" "SELECT template FROM sessions WHERE id='$sub'")
check "template 落库（explorer）" "explorer" "$tpl"
if grep -o '"name":"[a-z_]*"' "$SESS_DIR/$sub.jsonl" 2>/dev/null | grep -qE '"(write|edit)"'; then
  bad "explorer 只读约束" "出现 write/edit 调用（${sub}）"
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

# ── 5. kanban 建卡形状（todo/ 落盘、frontmatter id/created）──
T=$(mktemp -d)
kid=$(cd "$T" && KB_DIR="$T/kb" python3 "$TICKET_SH" new "e2e" -m "验收用")
if [ -n "$kid" ] && grep -q '^id: ' "$T"/kb/todo/$kid-*.md 2>/dev/null \
  && grep -q '^created: ' "$T"/kb/todo/$kid-*.md 2>/dev/null; then
  ok "kanban 建卡形状（todo/ 落盘、frontmatter id/created）"
else
  bad "kanban 建卡形状" "kid=$kid"
fi
rm -rf "$T"

# ── 6. session rules：spawn 时原文注入 system prompt，只作用当前会话 ──
# 模型复读暗号 = 规则真进了 system prompt 的铁证：jsonl 只存消息不存
# system prompt，user 提问不含暗号，assistant 答出即注入生效。
new_sid() { "$YOMI" rpc "$1" "$2" | tr -d '"'; }  # create/fork 返回裸 id 字符串
wait_idle() {  # $1=session id；首次消息含 spawn，轮询 phase=idle
  local i phase
  for i in $(seq 1 90); do
    phase=$("$YOMI" rpc get_session "{\"session_id\":\"$1\"}" 2>/dev/null \
      | python3 -c 'import json,sys; print(json.load(sys.stdin).get("phase",""))' 2>/dev/null)
    [ "$phase" = "idle" ] && return 0
    sleep 2
  done
  return 1
}
sid_a=$(new_sid create_session '{}')
sid_b=$(new_sid create_session '{}')
mkdir -p "$SESS_DIR/rules"
MARK_A="暗号紫气东来42号"
MARK_B="口令北斗七星7号"
printf '本话题守则：\n- 接头暗号：%s\n' "$MARK_A" > "$SESS_DIR/rules/$sid_a.md"
printf '本话题守则：\n- 接头暗号：%s\n' "$MARK_B" > "$SESS_DIR/rules/$sid_b.md"

"$YOMI" session send -s "$sid_a" "你的规则文件里的接头暗号是什么？只回答暗号本身，不要别的字。" >/dev/null 2>&1
wait_idle "$sid_a" || bad "session rules 会话 A 跑完" "超时未 idle"
grep -q "$MARK_A" "$SESS_DIR/$sid_a.jsonl" 2>/dev/null \
  && ok "session rules 注入（A 复读暗号）" || bad "session rules 注入（A 复读暗号）" "jsonl 未见暗号（${sid_a}）"

"$YOMI" session send -s "$sid_b" "你的规则文件里的接头暗号是什么？只回答暗号本身，不要别的字。" >/dev/null 2>&1
wait_idle "$sid_b" || bad "session rules 会话 B 跑完" "超时未 idle"
grep -q "$MARK_B" "$SESS_DIR/$sid_b.jsonl" 2>/dev/null \
  && ok "session rules 注入（B 复读暗号）" || bad "session rules 注入（B 复读暗号）" "jsonl 未见暗号（${sid_b}）"
grep -q "$MARK_A" "$SESS_DIR/$sid_b.jsonl" 2>/dev/null \
  && bad "session rules 隔离（B 不见 A 暗号）" "B 的 jsonl 出现 A 暗号（${sid_b}）" \
  || ok "session rules 隔离（B 不见 A 暗号）"

# fork 复制：确定性断言，无模型调用
child=$(new_sid fork_session "{\"parent_id\":\"$sid_a\",\"auto_approve_level\":\"caution\"}")
if [ -n "$child" ] && [ -f "$SESS_DIR/rules/$child.md" ] \
  && cmp -s "$SESS_DIR/rules/$sid_a.md" "$SESS_DIR/rules/$child.md"; then
  ok "fork 复制 rules 文件"
else
  bad "fork 复制 rules 文件" "child=$child 文件缺失或内容不同"
fi

echo
echo "== $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
