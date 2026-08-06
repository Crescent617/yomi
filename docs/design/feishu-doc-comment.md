# 设计文档：Feishu 云文档评论接入 —— 事件接收 + 文档内回复闭环

**Status:** Implemented（2026-08-06，Phase 1–4 完成，端到端闭环已真实验证）
**Date:** 2026-08-06

> **实施偏差**（相对初稿，代码已按此落地）：
> 1. 「跳过 bot 自发评论」放在 adapter 侧（bot 身份是 adapter 的领域知识；仅 `is_mentioned=true` 的事件才付出一次带缓存的 bot-info 调用），`comment.rs` 保留全部策略过滤。
> 2. `raw_text` 填裸评论文本（用于 session 标题），命令禁用由 hub 的 `message_command` 对 doc 消息强制返回 `None` 实现（斜杠开头的评论原样发给 agent）。
>
> **E2E 实证记录**（2026-08-06，真实租户验证）：
> - ✅ `batch_query` 用 tenant token 拉到真实评论：`is_whole`/`quote`/`reply_list.replies[].content.elements[]` 字段形态与解析代码一致；@bot 的 `person` 元素以 bot open_id 持久化；
> - ✅ 局部评论 thread 回复 API 成功；全文评论 replies API 报错 **1069302**（与参考实现一致），降级路径创建全文评论需 `reply_list` 包装（裸 `{content}` 报 9499——已修复）；
> - ✅ 文件级 subscribe API 接受 `drive.notice.comment_add_v1`（code 0）；
> - ✅ **端到端闭环全部通过**（控制台订阅 + 发布版本后）：评论 @bot → 事件到达（`is_mentioned=true`）→ meta 头注入（`[source: doc_comment]` + doc/comment_id/reply_id/quote 齐全）→ 局部评论 thread 内回复、全文评论走降级新开评论（均真实落文档）；同评论组追问复用同一 session（`reusing session`）、上下文连贯；非 @ 评论正确过滤（`bot not mentioned`）；bot 自己的评论不产生事件（源头防循环）。

---

## 1. 背景与问题

用户在飞书云文档（docx/sheet/bitable/…）上写评论协作（审阅、提问、行内反馈）时，无法直接调用 yomi。飞书开放平台提供事件 `drive.notice.comment_add_v1`：当文档上有新评论/评论回复且应用收到通知（典型场景：评论中 @ 了 bot）时推送给应用。

yomi 需要：接收该事件 → 拉取评论内容 → 作为 user 消息注入 agent 会话，**并在消息的 meta 头中明确告知 agent 这条消息来自某个飞书文档**（带齐 file_token / comment_id 等标识，agent 可据此用 lark-cli 等工具读文档、看完整评论串）→ agent 的最终回复**作为评论回复投递回文档内**，形成闭环。

当前代码的两个硬约束（与权限申请设计同构）：

- `feishu.rs::parse_event_json` 只放行 `im.message.receive_v1` / `drive.file.permission_member_applied_v1` / `card.action.trigger`，其余事件一律丢弃；
- 回复投递链路（`deliver_reply` / obs 状态卡 / typing）全部以 `external_chat_id` 为前提，而评论事件**没有 chat 上下文**。

## 2. 目标与非目标

**目标**

- 接收 `drive.notice.comment_add_v1` 事件（长连接，复用现有 ws 管道）；
- 仅处理**评论中 @ 了 bot** 的事件（`is_mentioned`）；跳过 bot 自己发的评论；
- 拉取评论内容（含局部评论的引用原文 quote），组装为 user 消息，meta 头标注飞书文档来源；
- **每个评论组（comment thread）一个 session**，同一评论组内的多轮追问共享上下文；
- agent 最终回复经评论回复 API 投递回文档（局部评论 → thread 内回复；全文评论 → 新全文评论）；
- 评论者维度的访问控制（复用 `allowed_users`/`blocked_users`）。

**非目标**

- 不处理非 @bot 的评论（应用作为文档 owner 收到的全部评论通知一律忽略）；
- 不在任何群聊投递状态卡/回执（评论会话是"无头"的，观测只靠 daemon 日志）；
- 不做评论串历史时间线注入（多轮连续性由 per-comment-thread session 提供；完整评论串 agent 可自行用工具拉取）；
- 不支持附件投递到评论（run 产出的 `<yomi_attachments>` 文件无法作为评论附件，仅记日志）；
- 不做"解决评论"（resolve）、评论表情回应等增强（见 §10 未来项）。

## 3. 前置条件（运维侧）

| 项 | 说明 |
|----|------|
| 事件订阅 | 开发者后台 → 事件订阅（长连接）→ 添加 `drive.notice.comment_add_v1`（**缺它事件帧根本不会推送，已实测**；可能需发布应用版本后生效）。此外已实测文件级订阅 API 接受该事件类型（`POST /drive/v1/files/{token}/subscribe?file_type=docx`，body `{"event_type":"drive.notice.comment_add_v1"}`），建议对每个目标文档调用一次 |
| 权限范围 | `docs:document.comment:read`（拉评论内容）、`docs:document.comment:create` 或 `docs:document.comment:write_only`（回评论）、`drive:drive.metadata:readonly`（取文档标题，权限申请功能已用同 API） |
| 触发方式 | 评论者在文档评论的 @ 选择器中 @ 到 bot（**可行性待实证**，§9 R1；官方有「Agent 回复云文档评论」场景文档，佐证支持） |
| 可见范围 | 应用需对文档有权限（owner/协作者），否则拉不到评论内容、也回不了评论 |

## 4. 已核实的飞书 API / 事件

### 4.1 事件 schema（`drive.notice.comment_add_v1`，v2 信封）

字段以官方 Python SDK（`lark_oapi.api.drive.v1.model.p2_drive_notice_comment_add_v1`）为准：

```json
{
  "header": { "event_type": "drive.notice.comment_add_v1", "create_time": "1720...(ms)", ... },
  "event": {
    "notice_meta": {
      "file_type": "docx",
      "file_token": "doxcnXXXX",
      "from_user_id": { "open_id": "ou_commenter" },   // 评论作者
      "to_user_id":   { "open_id": "ou_bot" },          // 被通知方（被 @ 者 / 相关方）
      "notice_type":  "add_comment"                     // 或 "add_reply"（取值集合待实证）
    },
    "comment_id": "7123456789",
    "reply_id":   "7123456790",
    "is_mentioned": true
  }
}
```

要点：

- **事件体不含评论正文**，只有 `comment_id` / `reply_id`，内容须调评论 API 拉取；
- `is_mentioned` = 评论中 @ 了 bot —— 本设计的触发条件；
- `notice_type` 过滤到 `add_comment` / `add_reply`（新建评论 / 评论内回复），其余（编辑、解决、删除等可能取值）忽略；
- 去重键：`{comment_id}:{reply_id}`（事件无 message_id 概念；ws 重投场景复用 `seen_messages` LRU，加前缀防撞 `om_` 系列）。

### 4.2 拉取评论内容（batch_query，**不能用单条 GET**）

```
POST /open-apis/drive/v1/files/{file_token}/comments/batch_query?file_type={file_type}&user_id_type=open_id
body: { "comment_ids": ["7123456789"] }
```

单条 `GET .../comments/{comment_id}` **只支持全文评论**；局部评论（划词评论，@bot 的典型形态）必须走 batch_query。返回 `FileComment`：

- `is_whole`：全文评论 / 局部评论；
- `quote`：局部评论引用的原文（行内锚点，关键上下文）；
- `reply_list.replies[]`：`{reply_id, user_id(open_id), create_time, content: {elements: [...]}}`；元素类型 `text_run`（`text`）/ `docs_link`（`url`）/ `person`（@ 用户，`user_id`）。用事件的 `reply_id` 定位触发那条回复。

### 4.3 回复评论（投递出口）

```
POST /open-apis/drive/v1/files/{file_token}/comments/{comment_id}/replies?file_type={file_type}&user_id_type=open_id
body: { "content": { "elements": [{ "type": "text_run", "text_run": { "text": "..." } }] } }
```

- **局部评论**：thread 内回复，上式即可；
- **全文评论**：replies API 报错（E2E 实测错误码 1069302），需降级为 `POST .../comments` 新加一条全文评论作为回复——注意其 body 为 `{"reply_list": {"replies": [{"content": ...}]}}` 包装形态（实测裸 `{content}` 报 9499）；
- 评论为纯文本（markdown 不渲染）；单条长度有限，参考实现按 4000 字符分片，本设计同样分片多条回复；
- 文档标题复用现有 `fetch_doc_title`（`drive/v1/metas/batch_query`）。

## 5. 核心设计

### 5.1 接收链路：新增 `ChannelEvent::DocCommentAdded`

`channels/mod.rs`：

```rust
pub enum ChannelEvent {
    Message(ChannelMessage),
    DocPermissionApplied(DocPermissionRequest),
    CardAction(CardAction),
    /// 飞书 `drive.notice.comment_add_v1`：文档评论 @ 了 bot。
    DocCommentAdded(DocCommentNotice),
}

/// 评论事件的原始载荷（ids only —— 内容拉取延后到策略过滤之后，
/// 与 image_keys 的 post-gate 延迟下载同一模式，避免被过滤的事件白烧 API）。
#[derive(Debug, Clone)]
pub struct DocCommentNotice {
    pub file_token: String,
    pub file_type: String,           // docx/sheet/bitable/...
    pub comment_id: String,
    pub reply_id: Option<String>,
    pub commenter_open_id: String,   // notice_meta.from_user_id
    pub is_mentioned: bool,
    pub notice_type: String,         // add_comment / add_reply / ...
    pub create_time: Option<i64>,    // header.create_time，unix ms
}
```

- `feishu.rs::parse_event_json` 的 `match` 加分支：解析 → 去重（`seen_messages` 键 `doc_comment:{comment_id}:{reply_id}`）→ 包装 `ChannelEvent::DocCommentAdded` 送出。**接收循环零 API 调用**（不取 token、不拉内容）；
- hub gate 循环新增分支（与 `DocPermissionApplied` 同构）：`tokio::spawn` 进 `channels/comment.rs` 处理，不堵串行循环。

### 5.2 策略过滤与内容组装（`channels/comment.rs`，新模块）

处理顺序（任一不过即终止，仅记日志）：

1. `notice_type ∈ {add_comment, add_reply}`；
2. `is_mentioned == true`（仅 @bot 的评论触发）；
3. `commenter_open_id != bot_open_id`（跳过 bot 自己回的评论，防自触发循环）；
4. 访问控制：仅用户维度 —— `commenter ∈ blocked_users` 拒绝；`allowed_users` 非空且不含 commenter 拒绝。**chat 维度不适用**（无 chat），不复用 `check_access`；
5. 拉取（`tokio::join!`）：`adapter.fetch_doc_comment(file_token, file_type, comment_id)`（§5.4，batch_query）+ `adapter.fetch_doc_title(...)`（现有）。拉取失败：带裸 meta 注入（正文为 `[评论内容拉取失败: ...]`），不静默丢；
6. 从返回的 `reply_list` 定位 `reply_id` 那条（`add_comment` 时即首条），抽取文本：`text_run` 拼接、`docs_link` 取 url、`person` 渲染为 `@user:{open_id}` 占位；
7. 组装 `ChannelMessage` 送入 `dispatch_tx`（串行 dispatch，保序复用现有链路）：

```rust
ChannelMessage {
    external_chat_id: String::new(),      // 无 chat —— 路由键与投递全靠 doc_comment
    external_user_id: commenter_open_id,
    external_message_id: None,            // 无 chat 消息 —— reaction/回执自然全部跳过
    is_mention: true,                     // 已按 @bot 过滤，必为 addressed
    raw_text: None,                       // 评论禁用斜杠命令（§5.5）；实施偏差：实为裸评论文本（喂 session 标题），命令由 hub `message_command` 对 doc 消息强制 None
    content: vec![ContentBlock::Text { text: assembled }],
    image_keys: vec![],                   // 评论元素无图片类型
    thread_id: None, root_id: None, parent_id: None,
    is_group: false,                      // 跳过历史注入/游标等群聊逻辑
    create_time: notice.create_time,
    doc_comment: Some(DocCommentRef { file_token, file_type, comment_id, reply_id }),
}
```

### 5.3 meta：user 消息头标注文档来源（本设计核心）

沿用现有 meta 头约定（adapter/hub 拼在 user 消息文本首行的 `[k: v]` 序列——`Message.metadata`/`_meta` 字段**不发给模型**，明确不使用）。组装的文本形态：

```
[2026-08-06 20:13:00][from_user_id: ou_xxx][platform: feishu][source: doc_comment]
[doc_title: 2026 产品方案][doc: docx:doxcnABC123][doc_url: https://feishu.cn/docx/doxcnABC123][comment_id: 7123456789][reply_id: 7123456790]
> 被划词引用的原文…（局部评论才有此行）

评论正文（@bot 占位已剔除）
```

- `[source: doc_comment]` 是与普通聊天消息的区分标记（普通消息无此键）；`[doc: {file_type}:{file_token}]` 与 `[doc_url]` 给 agent 读文档/回评论所需的全部标识；
- 无 `chat_id` 键（本来就没有），不伪造；
- 该 header 由 hub 侧组装（普通聊天消息的 header 在 adapter 拼，因为那里有事件原文；评论内容在 hub 拉取，故在 hub 拼）。

### 5.4 Adapter 新方法

```rust
impl FeishuAdapter {
    /// batch_query 拉一条评论（局部/全文通吃；单条 GET 只支持全文评论）。
    /// None = 评论不存在/已删除。
    async fn fetch_doc_comment(
        &self, file_token: &str, file_type: &str, comment_id: &str,
    ) -> Result<Option<DocCommentDetail>, ChannelError>;

    /// 回复评论：局部评论走 thread 回复；全文评论降级为新全文评论
    ///（replies API 对全文评论报错 1069302）。返回 reply_id/新 comment_id。
    /// 超长文本由调用方分片，本方法逐片调用。
    async fn reply_doc_comment(
        &self, file_token: &str, file_type: &str, comment_id: &str, text: &str,
    ) -> Result<Option<String>, ChannelError>;
}
```

`PlatformAdapter` trait 加对应默认实现（`fetch_doc_comment` → `Ok(None)`，`reply_doc_comment` → unsupported）；Telegram 不落地。

```rust
pub struct DocCommentDetail {
    pub is_whole: bool,
    pub quote: Option<String>,
    /// 评论组内全部回复（开放平台单页上限内），供定位 reply_id。
    pub replies: Vec<DocCommentReplyLite>, // {reply_id, user_id, create_time, text}
}
```

`ChannelMessage` 与 `SessionRouting` 增加同一类型：

```rust
/// 文档评论路由/来源：来自哪个评论组、回复投递到哪。
/// 触发 reply_id 只留在 DocCommentNotice 与 meta 头（投递永远面向评论组）。
#[derive(Debug, Clone, PartialEq)]
pub struct DocCommentRef {
    pub file_token: String,
    pub file_type: String,
    pub comment_id: String,
}
```

### 5.5 会话路由：每评论组一个 session

- mapping_key = `doc:{file_type}:{file_token}:{comment_id}`（`session_mapping_key` 顶部加分支：`msg.doc_comment.is_some()` 时返回该键，**优先于一切 chat/thread 规则**）。file_type/file_token/comment_id 均不含 `:`，可安全 split；
- `get_or_create_session` 的 `actual_chat_id` 存空串；**不加 DB 列、无 migration** —— `find_routing_by_session` 从 mapping_key 的 `doc:` 前缀解析出 `DocCommentRef` 填进 `SessionRouting.doc_comment`（reply_id 持久化意义不大，路由只需三元组）；
- 效果：同一评论组内的追问（`add_reply`，同 comment_id 新 reply_id）进同一 session，多轮连续；同一文档的不同评论组互不串味；
- 模型继承：`model_key_for_new_channel_session` 对非 chat key 会找父 chat session，此处 `mapping_key != chat_id` 且父键（空串）无 mapping → 自然回落到默认模型，无需特判；
- 斜杠命令：`raw_text = None` → `parse_channel_command` 得 `None`，全部评论内容直通 agent。`/stop` 等控制能力在评论通道缺失（可接受，见 §10）。

### 5.6 投递：deliver_reply 的 doc-comment 分支

`hub.rs::deliver_reply` 顶部：

```rust
if let Some(dc) = &routing.doc_comment {
    // 无状态卡可 morph、无 chat 可 flush —— 回复即评论回复。
    // 取 run 最终文本（reply buffering 已聚合），按 4000 字符分片逐条回。
    // 附件：评论 API 无附件概念，记 warn 并在末片附一行说明。
    // obs 内存态照常 settle（不含任何平台 I/O 的部分），保证状态不泄漏。
}
```

- 事件转发器（`start_event_forwarder`）：`routing.doc_comment.is_some()` 时跳过 `obs.handle_event`（状态卡）与 typing fallback，直接走 reply buffer 聚合 → `deliver_reply`；
- `notify_run_subscriptions`：评论会话无 chat、无订阅可能，mapping_key/chat 匹配天然落空，无需特判（防御性 skip 即可）；
- run 失败/取消：回复文本照常被投递（reply buffer 兜底逻辑不变），评论者能在文档内看到结果或错误。

### 5.7 配置

**无新增配置**。事件没订阅就没流量；触发固定为「评论 @bot」。成本控制用既有 `allowed_users`/`blocked_users`（用户维度）。若后续要放开「免 @ 触发」或加总开关，再加 `comment_require_mention`（见 §10）。

## 6. 数据流时序

```
评论者在文档评论中 @bot 提问
  │
  ▼ 飞书服务端（应用被 @，收到评论通知）
ws 长连接 ──► type:event 帧, event_type=drive.notice.comment_add_v1
  │
  ▼ feishu.rs::parse_event_json
去重(doc_comment:{comment_id}:{reply_id}) → ChannelEvent::DocCommentAdded
  │
  ▼ hub gate 循环（tokio::spawn，off-loop）
comment.rs：策略过滤（notice_type / is_mentioned / 非自发 / 用户黑白名单）
  ├─ 不过 ──► debug 日志，结束
  ▼
fetch_doc_comment(batch_query) ∥ fetch_doc_title
组装 meta 头 + quote + 评论正文 → ChannelMessage{doc_comment: Some(...)}
  │
  ▼ dispatch_tx（串行）
handle_incoming_message → prepare_trigger
mapping_key = doc:docx:{file_token}:{comment_id} → 新建/复用 session
kernel.send_steer(meta 头 + 正文)
  │
  ▼ agent run（可用 lark-cli 读文档全文/完整评论串，meta 已带齐标识）
  │
  ▼ run 结束，event forwarder 聚合 reply buffer
deliver_reply：routing.doc_comment 分支
  ├─ 局部评论 ──► POST .../comments/{comment_id}/replies（分片）
  └─ 全文评论 ──► POST .../comments（新全文评论）
评论者在文档内看到回复
```

## 7. 接口变化与影响范围

| 文件 | 变更 |
|------|------|
| `channels/mod.rs` | `ChannelEvent::DocCommentAdded`、`DocCommentNotice`、`DocCommentRef`、`DocCommentDetail`；`ChannelMessage`/`SessionRouting` 加 `doc_comment` 字段；`PlatformAdapter` 加 `fetch_doc_comment`/`reply_doc_comment`（默认不支持） |
| `channels/comment.rs`（新） | 策略过滤、内容拉取与文本抽取、meta 头组装、ChannelMessage 构造 |
| `channels/feishu.rs` | `parse_event_json` 加事件分支 + 去重；实现两个 adapter 方法（batch_query / replies / comments API） |
| `channels/hub.rs` | gate 循环新分支；`session_mapping_key` 加 doc 分支；`deliver_reply` 加评论投递分支；事件转发器跳过 obs/typing；`find_routing_by_session` 解析 mapping_key 回填 `SessionRouting.doc_comment`（store.rs 同步） |
| `docs/CONFIG.md` / `docs/config-schema.json` | 无配置变更；补一节「文档评论」运维前提（事件订阅 + scope）说明 |

GC 关联：评论会话是普通 session，`channel_session_mappings` 行随 `delete_by_sessions` 回收，无新账外数据。

## 8. 实施计划

1. **Phase 1（接收 + 过滤）**：`ChannelEvent` 新变体、`parse_event_json` 分支与去重、`comment.rs` 策略过滤。单测：事件 payload 解析（v2 信封）、各过滤条件（非 @、自发、黑名单）。
2. **Phase 2（内容 + meta + 会话）**：`fetch_doc_comment`（batch_query）+ 文本抽取 + meta 组装 + `session_mapping_key` doc 分支。单测：meta 文本形态（含/无 quote、docs_link/person 元素）、mapping_key 生成与往返解析。
3. **Phase 3（投递闭环）**：`reply_doc_comment`（含全文评论降级与分片）、`deliver_reply` 分支、事件转发器 obs/typing 跳过、`find_routing_by_session` 回填。`hub_test.rs` mock adapter 覆盖：局部评论回复、全文评论降级、多片、run 失败仍回评。
4. **Phase 4（E2E 实证）**：用 feishu-e2e skill——lark-cli 用户身份在真实文档评论 @bot，验证：事件到达与字段语义（§9 R1）、session 复用（同评论组追问）、回复落文档、meta 头正确性。同时实证全文评论回复错误码。

## 9. 风险与未决问题

| # | 风险 | 缓解 |
|---|------|------|
| R1 | **事件语义未经实证**：评论 @ 选择器能否 @ 到 bot、`is_mentioned`/`to_user_id` 确切含义、`notice_type` 全集、notice 事件是否需文件级 subscribe | Phase 4 第一步即实证（feishu-e2e）；过滤条件全部集中在 `comment.rs` 一处，实证偏差只改该模块 |
| R2 | 全文评论 replies API 报错（参考实现实测 1069302） | 按 `is_whole` 预选路径 + 捕获该错误码降级为新全文评论，双保险 |
| R3 | wiki 文档的 file_token 是节点 token，评论 API 可能需先解析真实 obj token | Phase 1 原样透传 file_type/file_token；wiki 场景报错记日志，列为未来项（wiki `get_node` 解析） |
| R4 | 同文档多人 @bot 并发评论，成本放大 | 触发面已收窄到「@bot 且过用户名单」；会话按评论组隔离互不影响；必要时加 `allowed_users` 收紧 |
| R5 | 评论回复长度/频率限制未明 | 4000 字符分片（参考实现同值）；API 错误逐片记 warn，不中断后续片 |
| R6 | 追问不 @bot 时 bot 不应答（严格触发策略的 UX 代价） | 与 IM 群聊 require_mention 语义一致，符合预期；未来可加「bot 已参与的评论组免 @」（§10） |
| R7 | mapping_key 编码承载路由信息（`doc:` 前缀解析）属约定而非 schema | 解析只认严格四段格式，畸形即落回普通 chat 路由（不会误投）；若未来评论路由信息增多，再迁移到独立 JSON 列 |

## 10. 未来项

- **bot 已参与评论组的免 @ 跟进**：评论组已有 session 时，后续 `add_reply` 免 `is_mentioned`（对齐 IM thread 内体验，需防误触发设计）；
- `comment_require_mention` 配置（放开全部评论触发）与功能总开关；
- 评论串时间线注入（`<comment_thread>` 块，需解决与 session 已有轮次的去重）；
- run 完成后自动「解决评论」（`PATCH .../comments/{comment_id}` solve）或表情回执（comment reaction API）；
- wiki 文档 token 解析（`GET /wiki/v2/spaces/get_node`）；
- `/stop` 等命令的评论通道等价物（如评论回复特定关键词）；
- 附件投递（评论不支持附件——可降级为上传云空间后回链接）。
