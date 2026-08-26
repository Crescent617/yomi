---
name: yomi-e2e
description: "yomi 隔离真机 E2E：起与生产完全隔离的测试 daemon，经 lark-cli 用户身份触发飞书事件，从日志 / session / sqlite 验证行为。当飞书通道或 daemon、CLI、cron 等改动需要真机端到端验证时使用。"
---

# Skill: yomi-e2e 隔离真机 E2E

## 1. 隔离环境

**三件套，每条 yomi 命令都带**（漏带即打到生产 daemon；`YOMI_DATA_DIR` 在 agent shell 里被注入为生产值且优先于 config，必须显式覆盖）：

```bash
YOMI_CONFIG="$HOME/.yomi/config-test.toml"   # test bot，app_id cli_a930282a6c78dbb4
YOMI_DATA_DIR="$HOME/.yomi-test"
YOMI_SOCKET=unix:///tmp/yomi-daemon-test.sock
```

```bash
# 起（restart 幂等：没在跑 = 直接起）
cd /Volumes/Data/repos/yomi && cargo build
<三件套> ./target/debug/yomi daemon restart

# 验（全过才继续）
<三件套> ./target/debug/yomi daemon status          # Daemon is running
grep 'Loaded 0 active cron jobs' ~/.yomi-test/logs/daemon.$(date +%Y-%m-%d).log
# 非 0 = 测试库遗留 job：sqlite 查 id、CLI pause（直改 db 打不到运行中的 scheduler）

# 拆（自清 socket/pid）
<三件套> ./target/debug/yomi daemon stop
```

观测：db `~/.yomi-test/yomi.db`、转录 `~/.yomi-test/sessions/<sid>.jsonl`、日志 `~/.yomi-test/logs/`。取 id 用 sqlite、改状态用 CLI。approval 群 `oc_4b1f6d93…`。绝不 `daemon restart` 生产 daemon。

## 2. 测飞书通道

bot 收不到自己消息的事件——**触发必须用用户身份（lark-cli）**；tenant token 只做只读验证。

### 触发

```bash
CHAT=oc_xxxxxxxx   # 目标群
BOT=ou_xxxxxxxx    # bot open_id（见下「凭据」）

lark-cli im +messages-send --chat-id $CHAT --text '内容' --jq '.data.message_id'
lark-cli im +messages-reply --message-id om_xxx --reply-in-thread --text '内容'   # reply-in-thread = 手动创建话题
```

@ bot：文本里写 `<at user_id="$BOT">名字</at>`。逃生舱：`lark-cli api GET /open-apis/...`。

### 凭据与只读 API

```bash
APP_ID=$(grep -A3 'type = "feishu"' ~/.yomi/config-test.toml | grep 'app_id' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
APP_SECRET=$(grep -A3 'type = "feishu"' ~/.yomi/config-test.toml | grep 'app_secret' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
TOKEN=$(curl -s -X POST 'https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal' \
  -H 'Content-Type: application/json' -d "{\"app_id\":\"$APP_ID\",\"app_secret\":\"$APP_SECRET\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["tenant_access_token"])')

curl -s 'https://open.feishu.cn/open-apis/bot/v3/info' -H "Authorization: Bearer $TOKEN"          # bot open_id
curl -s -G 'https://open.feishu.cn/open-apis/im/v1/messages' -H "Authorization: Bearer $TOKEN" \
  --data-urlencode 'container_id_type=thread' --data-urlencode 'container_id=omt_xxx' \
  --data-urlencode 'sort_type=ByCreateTimeAsc'  # 话题消息列表：含根消息，忽略 start_time
```

事件字段：话题内消息带 `thread_id`；`root_id` = 话题根；`parent_id` = 直接回复；顶层引用回复无 `thread_id`，`root_id` = `parent_id` = 被引用消息。

### yomi 侧观测

```bash
grep 'Feishu message' ~/.yomi-test/logs/daemon.$(date +%Y-%m-%d).log      # 事件到达与映射
sqlite3 ~/.yomi-test/yomi.db "SELECT external_chat_id, session_id, reply_msg_id FROM channel_session_mappings WHERE external_chat_id='<root_id>';"
python3 -c "
import json,sys
for l in open(sys.argv[1]):
    d=json.loads(l); s=json.dumps(d.get('content'),ensure_ascii=False)
    print(d.get('role'),'<quoted_message>' in s,'<recent_chat_history>' in s,s[:160])
" ~/.yomi-test/sessions/<sid>.jsonl      # 每条注入是否含 quoted / history 块
```

### 场景菜谱（每步间隔约 10 秒等 run 完成）

**A. 手动话题根必达**：发 M0（不 @bot，藏暗号）→ 话题回复 `@bot 暗号是什么`。预期：新 session 首条 user 消息含 `<quoted_message>`（M0 全文），bot 答出；话题内追问**无** quoted 块。
**B. 命令先行不认领话题**：M0 → 话题回复 `@bot /models` → mapping 行数 = 0 → 话题内再提问。预期：仍答出暗号；块顺序 history 前 quoted 后。
**C. bot 自建话题不重复注入**：顶层 `@bot 记住数字 N`（bot 回复开话题）→ 话题内追问。预期：无 quoted 块，凭 session 答出 N。
**D. 仓库集成测试**（真实 API、免发消息）：

```bash
YOMI_E2E_FEISHU_APP_ID="$APP_ID" YOMI_E2E_FEISHU_APP_SECRET="$APP_SECRET" \
YOMI_E2E_THREAD=omt_… YOMI_E2E_ROOT=om_… YOMI_E2E_TRIGGER=om_… \
cargo test -p kernel e2e_feishu -- --ignored --nocapture
```

**E. 非通道类（cron 等）**：CLI 驱动 + sqlite/jsonl 断言。以 cron per-run 为例：① create 省略 `--session` → get `session_id:null` + `session_template`；② 同 job trigger×2 → 两个不同新 session，各收各的消息；③ 排下一分钟 + max_runs 1 → 新 session、`run_count=1`、自动 completed；④ `--session` 绑定 → 消息追加进该 session、不新建；⑤ update 带 `--session` 绑定 / 不带解绑；⑥ 收尾删测试 job。

## 3. 排错对照

| 症状 | 先看 |
|---|---|
| bot 没反应 | 日志无 `Feishu message` → 事件没到（@ 错人 / 不在群 / 长连接没开） |
| 答非所问 | session 首条 user 消息根/引用在不在（不在 → 注入逻辑；在 → 模型） |
| 上下文重复 | 同一内容是否同现 quoted + history 两块（去重失效） |
