---
name: feishu-e2e
description: "隔离测试 yomi 上的 E2E 调试与验证：§0 起完全隔离的测试 daemon（test bot + ~/.yomi-test，一切 e2e 的基座，含 YOMI_DATA_DIR 陷阱）；飞书通道用 lark-cli（用户身份）触发真实消息事件，经日志 / session / sqlite 验证 yomi 行为。当调试飞书通道、话题、引用、历史注入问题，需要验证 bot 实际看到了什么，通道类功能（slash 命令、卡片、回复行为）发版前要真链路验证，或任何内核改动（cron、存储、调度）需要真机 e2e 时使用。"
---

# Skill: feishu-e2e 隔离 E2E 调试（飞书通道 + 内核行为）

bot 收不到自己消息的事件——**真实触发必须用用户身份**（lark-cli）；bot tenant token 只做只读验证；yomi 行为看日志 / session jsonl / sqlite。一切 e2e 先按 §0 起隔离测试 daemon。

## 0. 隔离测试 daemon：一切 e2e 的基座（必读）

所有 e2e 都在**完全隔离的测试 yomi** 上跑：test bot「Yomi 测试」（app_id `cli_a930282a6c78dbb4`）+ 数据目录 `~/.yomi-test` + 独立 unix socket。**绝不 `daemon restart` 生产 daemon 来做 e2e。**

```bash
cd /Volumes/Data/repos/yomi && cargo build

# 陷阱 1 防护：YOMI_DATA_DIR 必须显式覆盖（见下）
rm -f /tmp/yomi-test-daemon.sock
env YOMI_DATA_DIR=/Users/hrli/.yomi-test YOMI_SOCKET=/tmp/yomi-test-daemon.sock \
  ./target/debug/yomi daemon start -c ~/.yomi/config-test.toml \
  > /tmp/yomi-test-daemon.out 2>&1 &

# 验证隔离成功（两条都满足才继续）：
grep 'Feishu ws connected' /tmp/yomi-test-daemon.out
grep 'Loaded .* active cron jobs' /tmp/yomi-test-daemon.out   # 必须 Loaded 0（或只有测试 job）

# 客户端（CLI/RPC）全部走测试 socket：
export YOMI_SOCKET=/tmp/yomi-test-daemon.sock
./target/debug/yomi cron list        # 应只看到测试 job
```

关闭：`pkill -f 'target/debug/yomi daemon start'`；socket 残留先 `rm -f` 再重启。

**陷阱 1：`YOMI_DATA_DIR` 泄漏（2026-08-27 实踩）**。yomi 会给 spawn 的子进程注入 `YOMI_DATA_DIR=~/.yomi`（生产），且 **env 优先于 config 的 `data_dir`**。从 agent shell（daemon 的 tool 子进程）里起测试 daemon 而不显式覆盖，它会直接打开**生产库**——症状：启动日志 `Loaded N active cron jobs`（N=生产 active 数）；后果：测试 daemon 按点双跑生产 cron（03:33 dream、08:00 日报、shell 写生产 workspace）。交互 shell 无此注入，所以历史上没踩过；agent 环境必踩。

**陷阱 2：测试库会积累自己遗留的 cron job**。`~/.yomi-test` 不是生产快照（2026-08-19 独立创建，cron 表天然为空），但历次 e2e 留下的测试 job 会被 daemon 按点触发，污染本轮断言（如 session 计数、bus 消息序）。开工前清点，无关的一律 CLI pause（走 store.update + 通知 scheduler reload；**sqlite 直改打不到**运行中 scheduler 的内存缓存）：

```bash
sqlite3 ~/.yomi-test/yomi.db "SELECT id, name FROM cron_jobs WHERE status='active';" \
  | while read id; do ./target/debug/yomi cron pause "$id"; done
```

**观测点**（全在 `~/.yomi-test` 下，与生产无交集）：`yomi.db`（sessions / cron_jobs / channel_session_mappings）、`sessions/<sid>.jsonl`（转录，文件名=session id）、stdout `/tmp/yomi-test-daemon.out`。测试身份：approval 群 `oc_4b1f6d93…`；admin_users 含 hrli 主 app 与 lark-cli 两个 open_id（见 workspace memory/contacts.md）。模型 k3-hs 可用，agent turn 真实发生。

**小技巧**：`cron list` 表格别按空格分列（schedule 本身含空格）——取 id 用 sqlite 只读查询，改状态用 CLI。

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

从**测试 config** 取凭据（e2e 测的是 test bot，别用生产 config；**不要打印 secret**，输出一律脱敏）：

```bash
APP_ID=$(grep -A3 'type = "feishu"' ~/.yomi/config-test.toml | grep 'app_id' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
APP_SECRET=$(grep -A3 'type = "feishu"' ~/.yomi/config-test.toml | grep 'app_secret' | head -1 | sed -E 's/.*"([^"]+)".*/\1/')
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

- 评论读一致性：`batch_query` 的 `reply_list` 有秒~分钟级读延迟（新回复可能暂不可见）——**触发内容/时间线拉取必须用 `GET .../comments/{id}/replies`（list 端点，读新鲜）**，batch_query 只配取 `quote`/`is_whole`。验证回复落点也用 list 端点。评论 `create_time` 是秒级。
- 评论事件：bot 自己的评论/回复**不产生** `drive.notice.comment_add_v1`（notice 只推被通知方，源头防循环）；应用作为文档 owner 会收到**所有**评论的事件（`is_mentioned=false`，需自行过滤）。
- `container_id_type=thread` 的列表**包含话题根消息**，但**忽略 `start_time`**（别指望服务端按时间过滤）。
- 其他应用发的 schema 2.0 卡片，get-message 只回降级文本（"请升级至最新版本客户端"）。
- 事件字段：话题内消息带 `thread_id`；`root_id` = 话题根；`parent_id` = 直接回复的那条；顶层引用回复无 `thread_id`，`root_id` = `parent_id` = 被引用消息。

## 3. yomi 侧观测点

路径以**测试数据目录** `~/.yomi-test/` 为根（生产同名文件只作对照，别混用）；daemon stdout 在 `/tmp/yomi-test-daemon.out`：

```bash
# 事件与 session 映射（thread_id/root_id/is_mention；RIT 话题的 mapping_key 即 root_id）
grep 'Feishu message' /tmp/yomi-test-daemon.out
grep 'created session\|reusing session' /tmp/yomi-test-daemon.out | grep '<root_id>'

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

## 4. E2E 场景菜谱（附预期签名）

前置：按 §0 起隔离测试 daemon（build 含待验改动），`Feishu ws connected` + `Loaded 0 active cron jobs` 两条都见到。每步间隔约 10 秒等 run 完成。

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

**E. 非通道类 e2e（cron/存储/调度，2026-08-27 定型）**：同样走 §0 隔离 daemon，CLI 驱动 + sqlite/jsonl 断言，不碰飞书。以 cron per-run 语义为例的断言套餐：

1. 探针：`cron create` 省略 `--session` → get 里 `session_id: null` 且带 `session_template`（RPC 路径无 follow：cwd/project 缺省，权限=config 快照下限 caution）。
2. per-run：同一 job `cron trigger` 两次 → sqlite 里两个**不同** session，标题 `job名 · YYYY-MM-DD HH:MM:SS`，各收各的消息（jsonl 里 `{{date}}/{{time}}` 已渲染，agent 有回复）。
3. 真实调度：排下一分钟 + `--max-runs 1` → 到点后新 session 按触发时刻命名，`run_count=1`、`last_error=null`、status 自动 `completed`。
4. 绑定：`create --session <既有 sid>` → trigger 后消息落进该 sid 的转录（同会话追加 user 消息），**不产生新 session**。
5. update 三态：`update --message … --session <sid>` 绑定（模板清空）→ trigger 不新建；`update --message …`（不带 --session）解绑回 per-run（模板重抓）→ trigger 又新建。
6. 收尾：删掉测试 job；测试 session 按设计保留（keep）。

## 5. 排错线索对照

| 症状 | 先看 |
|---|---|
| bot 完全没反应 | 日志有没有 `Feishu message`（没有 → 事件没到：@ 错人 / 机器人不在群 / 长连接没开） |
| 收到了但答非所问 | session 首条 user 消息：根/引用内容在不在（不在 → 注入逻辑；在 → 模型问题） |
| 上下文重复 | 同一内容是否出现在 quoted + history 两个块（去重失效） |
| 怀疑平台行为 | §2 直接拉话题消息列表，对比事件字段 |
