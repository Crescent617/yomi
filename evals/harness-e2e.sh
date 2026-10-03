#!/usr/bin/env bash
# harness-e2e.sh — yomi harness 回归冒烟。
#
# 何时跑：改了 base prompt 装配、工具 desc/schema、内置模板（agent_tmpl/）、
# conductor、cron/存储语义之后。全是确定性断言（无 LLM judge）。
#
# 环境：自起隔离 daemon（独立 data_dir + 独立 socket + 剥离 [[channels]]
# 的 config），随时可跑，不碰生产 data_dir / 生产 daemon / 任何 IM 平台。
# config 从 ~/.yomi/config.toml 复制并删除全部 [[channels]] 块（保留模型
# 凭证等其余配置）；断言里的真实模型调用因此可用。
#
# 需要：target/debug/yomi（先 `cargo build -p cli`）；sqlite3。
# 用法：evals/harness-e2e.sh    （约 2-3 分钟，含 2 次真实模型调用）
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
YOMI="${YOMI:-$ROOT/target/debug/yomi}"
[ -x "$YOMI" ] || { echo "build CLI first: cargo build -p cli"; exit 2; }
command -v sqlite3 >/dev/null || { echo "need sqlite3"; exit 2; }

# ── 0. 隔离环境：data_dir + socket + 无渠道 config + 自建 daemon ──
# 从 yomi 会话里跑时进程环境自带注入的 YOMI_DATA_DIR/YOMI_SOCKET——
# 必须显式覆盖，否则 env override 盖过一切、打到生产上。
E2E="$(mktemp -d /tmp/yomi-harness-e2e.XXXXXX)"
DATA="$E2E/data"; mkdir -p "$DATA"
SOCK="$E2E/daemon.sock"
REAL_CONFIG="$HOME/.yomi/config.toml"
if [ -f "$REAL_CONFIG" ]; then
  # 删除 [[channels]] 起到下一个表头（含 [channels.platform] 子表）止的块。
  awk '/^\[\[channels\]\]/ { skip=1; next }
       skip && /^\[/ { skip=0 }
       !skip { print }' "$REAL_CONFIG" > "$E2E/config.toml"
else
  echo "WARN: no $REAL_CONFIG; isolated daemon has no model credentials" >&2
  printf '# empty\n' > "$E2E/config.toml"
fi
export YOMI_DATA_DIR="$DATA" YOMI_CONFIG="$E2E/config.toml" YOMI_SOCKET="unix://$SOCK"
DB="$DATA/yomi.db"
SESS_DIR="$DATA/sessions"

"$YOMI" daemon start >>"$E2E/daemon.log" 2>&1 &
# daemon 先 bind socket 再做存储初始化，所以 status 通 ≠ 服务就绪——
# 以 cron list 的 wire 往返（要真查 cron store）作为就绪信号。
for _ in $(seq 1 50); do
  "$YOMI" cron list >/dev/null 2>&1 && break
  sleep 0.2
done
if ! "$YOMI" cron list >/dev/null 2>&1; then
  echo "isolated daemon failed to start"; cat "$E2E/daemon.log"; rm -rf "$E2E"; exit 2
fi
cleanup() {
  "$YOMI" daemon stop >/dev/null 2>&1 || true
  rm -rf "$E2E"
}
trap cleanup EXIT

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); echo "PASS  $1"; }
bad() { FAIL=$((FAIL+1)); echo "FAIL  $1 — $2"; }
check() { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected [$2], got [$3]"; }

latest_sub() { sqlite3 "$DB" "SELECT id FROM sessions WHERE id LIKE 'sub_%' ORDER BY created_at DESC LIMIT 1"; }

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

# ── 1.6 message job 的 --work-dir 经 session_template 落库；绑
# --session 时 --work-dir 显式报错 ──
wid=$("$YOMI" cron create --name e2e-wd --schedule "0 9 * * *" --message "hi" --work-dir /tmp | grep -o 'cron_[A-Za-z0-9]*')
tpl=$(sqlite3 "$DB" "SELECT action FROM cron_jobs WHERE id='$wid'")
echo "$tpl" | grep -q '"working_dir":"/tmp"' \
  && ok "message job --work-dir 落库" || bad "message job --work-dir 落库" "$tpl"
"$YOMI" cron delete "$wid" >/dev/null 2>&1
if "$YOMI" cron create --name e2e-wd2 --schedule "0 9 * * *" --message "hi" --session sess_x --work-dir /tmp >/dev/null 2>&1; then
  bad "--session+--work-dir 显式报错" "未报错（job 已建）"
  wid2=$("$YOMI" cron list | awk '$2=="e2e-wd2" {print $1}')
  [ -n "$wid2" ] && "$YOMI" cron delete "$wid2" >/dev/null 2>&1
else
  ok "--session+--work-dir 显式报错"
fi

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

# ── 5. session rules：spawn 时原文注入 system prompt，只作用当前会话 ──
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

# ── 6. session wait：不存在的会话 exit 2；等待期间被删 exit 4 ──
"$YOMI" session wait -s sess_gone_404 --interval 1 >/dev/null 2>&1
check "session wait 不存在会话 exit 2" "2" "$?"

wsid=$("$YOMI" rpc create_session '{}' | tr -d '"')
"$YOMI" session send -s "$wsid" "写一份 300 行的详尽观察日记，每行一个独立句子，不许省略" >/dev/null 2>&1
# wait 先后台起跑完成首探（reachable_once=true），再删会话——
# 删早了会落进首探失败路径（exit 2）测不到 4。
"$YOMI" session wait -s "$wsid" --interval 1 >/dev/null 2>&1 &
wpid=$!
sleep 2
"$YOMI" rpc delete_session "{\"session_id\":\"$wsid\"}" >/dev/null 2>&1
wait $wpid
check "session wait 等待期间会话被删 exit 4" "4" "$?"

echo
echo "== $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
