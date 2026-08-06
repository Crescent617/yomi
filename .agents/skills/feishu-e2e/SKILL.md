---
name: feishu-e2e
description: "飞书通道 E2E 调试：用 lark-cli（用户身份）触发真实消息事件，经日志 / session / sqlite 验证 yomi 行为。当调试飞书通道、话题、引用、历史注入问题，或需要验证 bot 实际看到了什么时使用。"
---

# Skill: feishu-e2e 飞书通道 E2E 调试

bot 收不到自己消息的事件——**真实触发必须用用户身份**（lark-cli）；bot tenant token 只做只读验证；yomi 行为看日志 / session jsonl / sqlite。

## 1. lark-cli 触发事件

```bash
CHAT=oc_xxxxxxxx          # 目标群
BOT=ou_xxxxxxxx           # bot 的 open_id（获取见 §2）

# 发普通消息（拿 message_id）
lark-cli im +messages-send --chat-id $CHAT --text '内容' --jq '.data.message_id'

# 回复并开话题（对顶层消息 reply-in-thread 即"手动创建话题"，该消息成为话题根）
lark-cli im +messages-reply --message-id om_xxx --reply-in-thread --text '内容' --jq '.data.message_id'
```

- 文本中 @ bot：`<at user_id="$BOT">名字</at>`，服务端自动生成 mentions。
- 原始 API 逃生舱：`lark-cli api GET /open-apis/...`。

## 2. tenant token 只读验证

从 config 取凭据（**不要打印 secret**，输出一律脱敏）：

```bash
cd ~/.yomi
APP_ID=$(grep -A3 'type = "feishu"' config.toml | grep 'app_id' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
APP_SECRET=$(grep -A3 'type = "feishu"' config.toml | grep 'app_secret' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
TOKEN=$(curl -s -X POST 'https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal' \
  -H 'Content-Type: application/json' -d "{\"app_id\":\"$APP_ID\",\"app_secret\":\"$APP_SECRET\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["tenant_access_token"])')

# bot open_id（§1 的 $BOT）
curl -s 'https://open.feishu.cn/open-apis/bot/v3/info' -H "Authorization: Bearer $TOKEN" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["bot"]["open_id"])'

# 话题消息列表 / 单条消息（字段含 parent_id / root_id / thread_id）
curl -s -G 'https://open.feishu.cn/open-apis/im/v1/messages' -H "Authorization: Bearer $TOKEN" \
  --data-urlencode 'container_id_type=thread' --data-urlencode 'container_id=omt_xxx' \
  --data-urlencode 'sort_type=ByCreateTimeAsc' --data-urlencode 'page_size=50'
curl -s "https://open.feishu.cn/open-apis/im/v1/messages/om_xxx" -H "Authorization: Bearer $TOKEN"
```

**平台行为事实**（文档里看不出来的）：

- 评论读一致性：`batch_query` 的 `reply_list` 有秒~分钟级读延迟（新回复可能暂不可见）；验证评论回复要用 `GET .../comments/{id}/replies`（list，及时）。评论 `create_time` 是秒级。
- 评论事件：bot 自己的评论/回复**不产生** `drive.notice.comment_add_v1`（notice 只推被通知方，源头防循环）；应用作为文档 owner 会收到**所有**评论的事件（`is_mentioned=false`，需自行过滤）。
- `container_id_type=thread` 的列表**包含话题根消息**，但**忽略 `start_time`**（别指望服务端按时间过滤）。
- 其他应用发的 schema 2.0 卡片，get-message 只回降级文本（"请升级至最新版本客户端"）。
- 事件字段：话题内消息带 `thread_id`；`root_id` = 话题根；`parent_id` = 直接回复的那条；顶层引用回复无 `thread_id`，`root_id` = `parent_id` = 被引用消息。

## 3. yomi 侧观测点

```bash
# 事件与 session 映射（thread_id/root_id/is_mention；RIT 话题的 mapping_key 即 root_id）
grep 'Feishu message' ~/.yomi/logs/daemon.$(date +%Y-%m-%d).log
grep 'created session\|reusing session' ~/.yomi/logs/daemon.$(date +%Y-%m-%d).log | grep '<root_id>'

# session 实际注入的上下文（每条消息是否含 quoted / history 块）
python3 -c "
import json,sys
for l in open(sys.argv[1]):
    d=json.loads(l); s=json.dumps(d.get('content'),ensure_ascii=False)
    print(d.get('role'),'<quoted_message>' in s,'<recent_chat_history>' in s,s[:160])
" ~/.yomi/sessions/<sid>.jsonl

# mapping / 历史游标
sqlite3 ~/.yomi/yomi.db "SELECT external_chat_id, session_id, reply_msg_id FROM channel_session_mappings WHERE external_chat_id='<root_id>';"
sqlite3 ~/.yomi/yomi.db "SELECT * FROM channel_history_cursors;"
```

## 4. E2E 场景菜谱（附预期签名）

前置：`cargo build && ./target/debug/yomi daemon restart`，日志出现 `Feishu ws connected`。每步间隔约 10 秒等 run 完成。

**A. 手动话题根必达**：发 M0（不 @bot，内容藏暗号）→ 话题回复 `@bot 暗号是什么`。
预期：新 session 首条 user 消息含 `<quoted_message>`（M0 全文），bot 答出暗号；话题内追问的 user 消息**无** quoted 块（不重复注入）。

**B. 命令先行不认领话题**：发 M0 → 话题回复 `@bot /models` → 查 db：root_id 的 mapping 行数 = 0 → 话题内再提问。
预期：bot 仍答出暗号（根经 quoted 注入）；块顺序 history 在前、quoted 在后。

**C. bot 自建话题不重复注入**：顶层 `@bot 记住数字 N`（bot 回复开话题）→ 话题内追问 `数字是几`。
预期：追问的 user 消息**无** quoted 块，bot 凭 session 答出 N。

**D. 仓库内集成测试**（真实 API、免发消息，`hub_test.rs` 底部两个 `#[ignore]` 测试）：

```bash
YOMI_E2E_FEISHU_APP_ID="$APP_ID" YOMI_E2E_FEISHU_APP_SECRET="$APP_SECRET" \
YOMI_E2E_THREAD=omt_… YOMI_E2E_ROOT=om_… YOMI_E2E_TRIGGER=om_… \
cargo test -p kernel e2e_feishu -- --ignored --nocapture
```

## 5. 排错线索对照

| 症状 | 先看 |
|---|---|
| bot 完全没反应 | 日志有没有 `Feishu message`（没有 → 事件没到：@ 错人 / 机器人不在群 / 长连接没开） |
| 收到了但答非所问 | session 首条 user 消息：根/引用内容在不在（不在 → 注入逻辑；在 → 模型问题） |
| 上下文重复 | 同一内容是否出现在 quoted + history 两个块（去重失效） |
| 怀疑平台行为 | §2 直接拉话题消息列表，对比事件字段 |
