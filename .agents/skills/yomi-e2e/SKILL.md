---
name: yomi-e2e
description: "yomi 隔离真机 E2E：起与生产完全隔离的测试 daemon（config-test.toml + ~/.yomi-test + 独立 socket）；飞书通道经 lark-cli 用户身份触发，行为经日志 / session jsonl / sqlite 验证。当通道类改动（slash 命令、卡片、回复行为）发版前需真链路验证，或内核改动（cron、存储、调度）需要真机 e2e 时使用。"
---

# Skill: yomi-e2e 隔离真机 E2E

## 1. 隔离环境

**三件套，每条 yomi 命令都带**（shell 环境不跨调用保留，漏带即打到生产 daemon）：

```bash
YOMI_CONFIG="$HOME/.yomi/config-test.toml"   # test bot「Yomi 测试」app_id cli_a930282a6c78dbb4
YOMI_DATA_DIR="$HOME/.yomi-test"             # agent shell 里该变量被注入成生产值且优先于 config 的 data_dir，必须显式覆盖
YOMI_SOCKET=unix:///tmp/yomi-daemon-test.sock
```

**起**：

```bash
cd /Volumes/Data/repos/yomi && cargo build   # 用被测二进制；已 build 可跳过
<三件套> ./target/debug/yomi daemon restart  # 幂等：没在跑 = 直接起
```

**验**（全过才继续）：

```bash
<三件套> ./target/debug/yomi daemon status   # Daemon is running
ls /tmp/yomi-daemon-test.sock
grep 'Loaded 0 active cron jobs' ~/.yomi-test/logs/daemon.$(date +%Y-%m-%d).log
```

`Loaded N` 非 0 → 测试库遗留 job，逐个 CLI pause（sqlite 直改打不到运行中 scheduler 的内存缓存）：
`sqlite3 ~/.yomi-test/yomi.db "SELECT id FROM cron_jobs WHERE status='active';" | while read id; do <三件套> ./target/debug/yomi cron pause "$id"; done`

**用**：CLI 带三件套即打到测试 daemon（`cron` / `session list` / `events --all` / `run "prompt"`）。GUI 读 `YOMI_SOCKET`。**绝不 `daemon restart` 生产 daemon。**

**拆**：`<三件套> ./target/debug/yomi daemon stop`（graceful stop 自清 socket/pid）。

**观测点**：db `~/.yomi-test/yomi.db`、转录 `~/.yomi-test/sessions/<sid>.jsonl`、日志 `~/.yomi-test/logs/daemon.<date>.log`。取 id 用 sqlite 只读查询、改状态用 CLI（`cron list` 表格含空格，不可按列解析）。测试 approval 群 `oc_4b1f6d93…`，admin_users 见 workspace memory/contacts.md；模型 k3-hs 可用，agent turn 真实发生。

## 2. 测飞书通道

bot 收不到自己消息的事件——**触发必须用用户身份（lark-cli）**；tenant token 只做只读验证。

### 2.1 触发事件

```bash
CHAT=oc_xxxxxxxx          # 目标群
BOT=ou_xxxxxxxx           # bot open_id（获取见 2.2）

# 发普通消息（拿 message_id）
lark-cli im +messages-send --chat-id $CHAT --text '内容' --jq '.data.message_id'

# 回复并开话题（对顶层消息 reply-in-thread 即"手动创建话题"，该消息成为话题根）
lark-cli im +messages-reply --message-id om_xxx --reply-in-thread --text '内容' --jq '.data.message_id'
```

- 文本中 @ bot：`<at user_id="$BOT">名字</at>`，服务端自动生成 mentions。
- 原始 API 逃生舱：`lark-cli api GET /open-apis/...`。

### 2.2 tenant token 只读验证

从 `~/.yomi/config-test.toml` 取凭据（**不要打印 secret**，输出一律脱敏）：

```bash
APP_ID=$(grep -A3 'type = "feishu"' ~/.yomi/config-test.toml | grep 'app_id' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
APP_SECRET=$(grep -A3 'type = "feishu"' ~/.yomi/config-test.toml | grep 'app_secret' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
TOKEN=$(curl -s -X POST 'https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal' \
  -H 'Content-Type: application/json' -d "{\"app_id\":\"$APP_ID\",\"app_secret\":\"$APP_SECRET\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["tenant_access_token"])')

# bot open_id（2.1 的 $BOT）
curl -s 'https://open.feishu.cn/open-apis/bot/v3/info' -H "Authorization: Bearer $TOKEN" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["bot"]["open_id"])'

# 话题消息列表 / 单条消息（字段含 parent_id / root_id / thread_id）
curl -s -G 'https://open.feishu.cn/open-apis/im/v1/messages' -H "Authorization: Bearer $TOKEN" \
  --data-urlencode 'container_id_type=thread' --data-urlencode 'container_id=omt_xxx' \
  --data-urlencode 'sort_type=ByCreateTimeAsc' --data-urlencode 'page_size=50'
curl -s "https://open.feishu.cn/open-apis/im/v1/messages/om_xxx" -H "Authorization: Bearer $TOKEN"
```

**平台行为事实**（文档里看不出来的）：

- 评论读一致性：`batch_query` 的 `reply_list` 有秒~分钟级读延迟——**触发内容/时间线拉取必须用 `GET .../comments/{id}/replies`（list 端点，读新鲜）**，batch_query 只配取 `quote`/`is_whole`。验证回复落点也用 list 端点。评论 `create_time` 是秒级。
- 评论事件：bot 自己的评论/回复**不产生** `drive.notice.comment_add_v1`（notice 只推被通知方）；应用作为文档 owner 会收到**所有**评论的事件（`is_mentioned=false`，需自行过滤）。
- `container_id_type=thread` 的列表**包含话题根消息**，但**忽略 `start_time`**。
- 其他应用发的 schema 2.0 卡片，get-message 只回降级文本。
- 事件字段：话题内消息带 `thread_id`；`root_id` = 话题根；`parent_id` = 直接回复的那条；顶层引用回复无 `thread_id`，`root_id` = `parent_id` = 被引用消息。

### 2.3 yomi 侧观测

```bash
# 事件与 session 映射（thread_id/root_id/is_mention；RIT 话题的 mapping_key 即 root_id）
grep 'Feishu message' ~/.yomi-test/logs/daemon.$(date +%Y-%m-%d).log
grep 'created session\|reusing session' ~/.yomi-test/logs/daemon.$(date +%Y-%m-%d).log | grep '<root_id>'

# session 实际注入的上下文（每条消息是否含 quoted / history 块）
python3 -c "
import json,sys
for l in open(sys.argv[1]):
    d=json.loads(l); s=json.dumps(d.get('content'),ensure_ascii=False)
    print(d.get('role'),'<quoted_message>' in s,'<recent_chat_history>' in s,s[:160])
" ~/.yomi-test/sessions/<sid>.jsonl

# mapping / 历史游标
sqlite3 ~/.yomi-test/yomi.db "SELECT external_chat_id, session_id, reply_msg_id FROM channel_session_mappings WHERE external_chat_id='<root_id>';"
sqlite3 ~/.yomi-test/yomi.db "SELECT * FROM channel_history_cursors;"
```

### 2.4 场景菜谱（附预期签名）

每步间隔约 10 秒等 run 完成。

**A. 手动话题根必达**：发 M0（不 @bot，内容藏暗号）→ 话题回复 `@bot 暗号是什么`。
预期：新 session 首条 user 消息含 `<quoted_message>`（M0 全文），bot 答出暗号；话题内追问的 user 消息**无** quoted 块。

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

**E. 非通道类（cron/存储/调度）**：CLI 驱动 + sqlite/jsonl 断言，不碰飞书。以 cron per-run 语义为例：

1. 探针：`create` 省略 `--session` → get 里 `session_id: null` 且带 `session_template`（RPC 路径 cwd/project 缺省，权限=config 快照下限 caution）。
2. per-run：同 job `trigger` 两次 → 两个**不同** session（标题 `job名 · YYYY-MM-DD HH:MM:SS`），各收各的消息（jsonl 里模板变量已渲染、agent 有回复）。
3. 真实调度：排下一分钟 + `--max-runs 1` → 新 session 按触发时刻命名，`run_count=1`、`last_error=null`、status 自动 `completed`。
4. 绑定：`create --session <既有 sid>` → trigger 后消息追加进该 sid 转录，**不产生新 session**。
5. update 三态：`update --message … --session <sid>` 绑定（模板清空）→ trigger 不新建；`update --message …`（不带 --session）解绑（模板重抓）→ trigger 又新建。
6. 收尾删掉测试 job；测试 session 按设计保留（keep）。

## 3. 排错线索对照

| 症状 | 先看 |
|---|---|
| bot 完全没反应 | 日志有没有 `Feishu message`（没有 → 事件没到：@ 错人 / 机器人不在群 / 长连接没开） |
| 收到了但答非所问 | session 首条 user 消息：根/引用内容在不在（不在 → 注入逻辑；在 → 模型问题） |
| 上下文重复 | 同一内容是否出现在 quoted + history 两个块（去重失效） |
| 怀疑平台行为 | 2.2 直接拉话题消息列表，对比事件字段 |
