# 设计文档：Feishu 云文档权限申请受理 —— 事件接收 + 命令/按钮审批

**Status:** Implemented（2026-07-30，含 DM 兜底与交互卡片按钮）
**Date:** 2026-07-23（2026-07-30 修订：通知投递改为「管理群 / DM 兜底」，明确不做自动建群；交互卡片按钮纳入范围——`card.action.trigger` 已确认可走长连接）

---

## 1. 背景与问题

yomi 以应用身份（tenant_access_token）通过 `docx` 等 API 创建飞书云文档时，**文档所有者是应用本身**。其他用户打开文档链接点「申请权限」后，申请通知只存在于飞书客户端的通知中心（发给"所有者"——即应用，而应用没有人盯着），申请人永远等不到结果。

飞书开放平台为此提供事件 `drive.file.permission_member_applied_v1`：当有人申请文件协作者权限时推送给应用。yomi 需要：接收该事件 → 通知管理员 → 管理员批准后调用 OpenAPI 授权 → 反馈结果。

当前代码的两个硬约束：

- `feishu.rs::parse_event_json` 只放行 `im.message.receive_v1`，其余事件一律丢弃；
- 入站通道 `mpsc::Sender<ChannelMessage>` 只承载"聊天消息"，没有承载平台事件的类型。

## 2. 目标与非目标

**目标**

- 接收飞书云文档权限申请事件，持久化为待审批记录；
- 格式化为卡片通知到**指定管理群**；未配置管理群时 **DM 兜底**（私聊逐个通知管理员）；**不做自动建群**；
- 管理员点卡片按钮（批准/拒绝）或发命令批准（可改权限级别）/ 拒绝；
- 批准后申请人收到飞书系统通知；审批结果回写通知卡片；
- 并发审批只生效一次，全程有审计（谁、何时、批了什么）。

**非目标**

- ~~不做卡片交互按钮~~（2026-07-30 修订：已核实 `card.action.trigger` 支持**长连接**接收——开发者后台「回调配置」选长连接、添加「卡片回传交互」即可，官方 SDK 的 `larkws` 客户端有完整示例。yomi 自实现 ws 客户端可直接解析数据帧，无需 HTTP webhook）。
- 不做审批流（飞书 Approval 应用对接）。
- 不主动 DM 申请人告知结果（批准由飞书系统 `need_notification` 通知；拒绝不通知，与飞书原生行为一致）。
- 不处理"机器人自身的工具执行权限"（kernel `AgentEvent::PermissionRequest`）——那是另一条链路，channel session 目前是 `auto_approve_level: Dangerous`，与本设计无关。

## 3. 前置条件（运维侧）

| 项 | 说明 |
|----|------|
| 事件订阅 | 开发者后台 → 事件订阅 → 添加 `drive.file.permission_member_applied_v1`（经长连接接收 ✓ 已实测验证）。**注意**：云文档事件为两级订阅——除控制台添加事件外，还必须对每个文档调用文件级订阅 API（`POST /drive/v1/files/{token}/subscribe?file_type=docx`，body `{"event_type":"drive.file.permission_member_applied_v1"}`），否则服务端不生成事件（实测：仅控制台订阅时推送记录为空）。bot 侧自动订阅见 §10 未来项 |
| 回调订阅 | 开发者后台 → 回调订阅（长连接）→ 添加 `card.action.trigger`（卡片回传交互）；**需发布应用版本后生效** |
| 权限范围 | 云文档相关 scope（查看、评论、编辑和管理云空间中所有文件），及增加协作者所需范围 |
| 推送条件 | **仅当应用是文档的所有者/管理者时才收到该事件**。用户私人文档的申请不会推给应用——本设计只覆盖"应用拥有的文档"场景 |

## 4. 已核实的飞书 API / 事件

### 4.1 事件 schema（`drive.file.permission_member_applied_v1`）

```json
{
  "header": { "event_type": "drive.file.permission_member_applied_v1", ... },
  "event": {
    "file_type": "docx",
    "file_token": "doxcnXXXX",
    "operator_id":               { "union_id": "...", "user_id": "...", "open_id": "ou_xxx" },
    "approver_id":               { ... },
    "application_user_list":     [{ "open_id": "ou_aaa", ... }],
    "application_chat_list":     ["oc_bbb"],
    "application_department_list": ["od_ccc"],
    "application_remark": "求权限看下方案",
    "permission": "view"
  }
}
```

申请人可能是三类：**用户**（`application_user_list`，用 `open_id`）、**群**（`application_chat_list`）、**部门**（`application_department_list`）；`permission ∈ view | edit | full_access`。

### 4.2 授权 API（批准时调用）

```
POST /open-apis/drive/v1/permissions/:token/members
params: type=<file_type>&need_notification=true
body: {
  "member_type": "openid" | "openchat" | "opendepartmentid",
  "member_id":   "ou_xxx" | "oc_xxx"  | "od_xxx",
  "perm":        "view" | "edit" | "full_access",
  "type":        "user" | "chat" | "department"     // 与 member_type 对应
}
```

- 多申请人可用 `POST .../members/batch_create`（`members: [...]`，同结构数组）一次提交；
- `need_notification=true`：批准后申请人收到飞书系统通知；
- **拒绝无对应 API**：不批准即为拒绝，仅本地标记；
- 配套：`DELETE .../members/:member_id`（事后回收权限，留作未来命令）。

## 5. 核心设计

### 5.1 接收链路：`ChannelEvent` 枚举

`channels/mod.rs` 将入站类型从 `ChannelMessage` 泛化为枚举：

```rust
/// 平台入站载荷：聊天消息 / 平台事件 / 卡片按钮回调
#[derive(Debug, Clone)]
pub enum ChannelEvent {
    Message(ChannelMessage),
    DocPermissionApplied(DocPermissionRequest),
    CardAction(CardAction),
}

#[derive(Debug, Clone)]
pub struct DocPermissionRequest {
    pub file_token: String,
    pub file_type: String,              // docx/sheet/bitable/...
    pub permission: String,             // view/edit/full_access
    pub remark: Option<String>,
    pub applicant_users: Vec<String>,   // open_id 列表
    pub applicant_chats: Vec<String>,   // chat_id 列表
    pub applicant_departments: Vec<String>,
}

/// 卡片按钮回调（card.action.trigger）：value 里携带审批动作与申请 id
#[derive(Debug, Clone)]
pub struct CardAction {
    pub operator_open_id: String,
    pub chat_id: Option<String>,        // 回调上下文中的会话（反馈消息用）
    pub value: serde_json::Value,       // {"action": "approve"|"deny", "id": N}
}
```

- `PlatformAdapter::run_receiver` 签名改为 `mpsc::Sender<ChannelEvent>`；Telegram 只构造 `Message` 变体，改动最小。
- `feishu.rs::parse_event_json` 的 `event_type` 匹配从单一放行改为 `match`：`im.message.receive_v1` 走现有逻辑；`drive.file.permission_member_applied_v1` 解析后包装为 `ChannelEvent::DocPermissionApplied`；其余仍忽略。
- ws 数据帧按 header `type` 分发：`event` 走事件解析；`card` 解析为 `ChannelEvent::CardAction`（见 §5.7）；未知类型记 debug 日志（协议实证用，见 R8）。
- hub 处理循环 `match` 各分支：`Message` 走现有逻辑；`DocPermissionApplied` 走 §5.3 通知逻辑；`CardAction` 走 §5.7 按钮审批。**平台事件与回调不做 `check_access`/`require_mention` 过滤**（它们不是用户消息；回调另有 `admin_users` 鉴权）。

### 5.2 持久化：待审批表

`storage/migrations.rs` 新增 migration：

```sql
CREATE TABLE channel_doc_permission_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_name TEXT NOT NULL,
    file_token TEXT NOT NULL,
    file_type TEXT NOT NULL,
    permission TEXT NOT NULL,
    remark TEXT,
    applicant_users TEXT NOT NULL DEFAULT '[]',   -- JSON 数组
    applicant_chats TEXT NOT NULL DEFAULT '[]',
    applicant_departments TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'pending',        -- pending/approved/denied
    notify_msg_ids TEXT NOT NULL DEFAULT '[]',       -- 通知卡片 message_id 列表（JSON 数组；群模式 1 条，DM 兜底每位管理员 1 条）
    resolved_by TEXT,                              -- 审批人 open_id
    resolved_perm TEXT,                            -- 实际授予的权限
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    resolved_at TIMESTAMP
);
```

`ChannelStore` trait 增加：

```rust
/// 落库（含查重）：同一申请仍有 pending 记录时返回 None（ws 重投去重）
async fn save_perm_request(&self, channel: &str, req: &DocPermissionRequest) -> KernelResult<Option<i64>>;
async fn set_perm_notify_msgs(&self, id: i64, msg_ids: &[String]) -> KernelResult<()>;
async fn list_pending_perm_requests(&self, channel: &str) -> KernelResult<Vec<PermRequestRow>>;
/// 条件更新：仅当 status='pending' 时翻转，返回是否抢到（并发审批只赢一次）
async fn resolve_perm_request(
    &self, id: i64, status: &str, resolved_by: &str, resolved_perm: Option<&str>,
) -> KernelResult<Option<PermRequestRow>>;
/// 授权 API 失败后的逆操作：恢复 pending，清空审批字段
async fn reopen_perm_request(&self, id: i64) -> KernelResult<()>;
```

短 id（自增整数）即管理员命令引用的编号。过期不做主动清理：`/permits` 列表展示创建时间，由人判断（飞书侧申请长期不处理自动失效，与本地状态无一致性强依赖）。

### 5.3 通知：管理群 / DM 兜底

hub 收到 `DocPermissionApplied`：

1. 查重（R2）后 `save_perm_request` 落库，拿到 `id`；
2. 格式化卡片（schema 2.0，橙 header，**底部一排按钮**）：

```
┌ 📄 文档权限申请 #3 ───────────────┐
│ 申请人  <at id=ou_aaa></at>（渲染为可点击姓名）│
│ 文档    [求权限方案](链接，取文档真实标题)      │
│ 申请权限 view                       │
│ 备注    求权限看下方案                │
│ [ ✅ 批准 ] [ ❌ 拒绝 ]（column_set 一排）  │
│ 改权限批准: /approve 3 [..]（灰色小字）  │
└────────────────────────────────────┘
```

布局要点（均已实测）：schema 2.0 中 `button` 直接作 body 元素（v1 `action` 容器已废弃，误用报 200861）；`column_set` 双列实现按钮一排；申请人用 `<at id=open_id>` 渲染为可点击姓名；文档标题通过 `POST /drive/v1/metas/batch_query` 实时获取（取不到回退 file_token）。按钮（card kit 2.0 `behaviors: [{type: "callback", value: ...}]`）：批准（primary）value `{"action":"approve","id":3}`，拒绝（danger）value `{"action":"deny","id":3}`。批准按申请权限授予；**改权限级别仍走命令**（按钮不承载参数选择）。终态卡片移除按钮。

3. 投递（二选一）：
   - 配了 `approval_chat_id` → 发一张卡片到该管理群；
   - 未配置 → **DM 兜底**：对 `admin_users` 逐个以 `receive_id_type=open_id` 发私聊卡片（审批命令在私聊中同样可用，hub 的 p2p 路径原生支持）。两者都未配置则功能关闭，收到事件仅记日志；
4. 发送成功后 `set_perm_notify_msgs` 记录全部 `message_id`（群模式 1 条、DM 模式 N 条），供审批后**统一回写所有卡片**，保证每位管理员看到的都是终态。

文档链接按 `file_type` 拼接：`https://feishu.cn/{file_type}/{file_token}`（docx/sheet/bitable 等同构）。

### 5.4 审批命令

新增 channel 命令（加入 `hub.rs::CMD_PREFIXES`，注意保持最长前缀优先）：

| 命令 | 语义 |
|------|------|
| `/permits` | 列出本 channel 全部 pending 申请（编号、申请人、文档、权限、时间） |
| `/approve <id> [perm]` | 批准 #id；`perm` 缺省用申请值，可显式覆盖为 `view`/`edit`/`full_access` |
| `/deny <id>` | 拒绝 #id（仅本地标记） |

**审批流程（`/approve`）**：

```
解析 id/perm ──► resolve_perm_request(id, approved, admin_open_id, perm)
  │               （条件 UPDATE，抢状态）
  ├─ 未抢到 ──► 回复 "该申请已被处理 / 不存在"
  ▼ 抢到
FeishuAdapter::grant_doc_permission(file_token, file_type, applicants, perm)
  │  逐类映射 member_type: user→openid / chat→openchat / department→opendepartmentid
  │  多申请人走 batch_create；need_notification=true
  ├─ API 失败 ──► 回滚 status 为 pending（resolve 逆操作）+ 回复错误
  ▼
update_card(逐个 notify_msg_ids, 终态卡片)  // 绿："✅ #3 已批准 view · by ou_admin"
+ 回复确认消息（线程内）
```

`/deny`：`resolve_perm_request(id, denied, ...)` 成功即回写所有通知卡片（灰："❌ #3 已拒绝 · by ou_admin"），不调任何飞书 API。

**通知卡片回写**复用可观测性设计的 `PlatformAdapter::update_card`（见 `feishu-channel-observability.md`）；若该能力未落地，退化为追加一条结果消息，不阻塞本设计。

### 5.5 配置

`ChannelConfig` 增加（`snake_case`，同步 `docs/config-schema.json` 与 `docs/CONFIG.md`）：

```rust
/// 文档权限申请通知目标群 chat_id；缺省时 DM 兜底（私聊通知 admin_users）；
/// 与 admin_users 同时缺省则该功能关闭（收到事件仅记日志）。不做自动建群。
#[serde(default)]
pub approval_chat_id: Option<String>,
/// 有审批权的管理员 open_id 列表；缺省则任何人不得审批，且无管理群时 DM 兜底无收件人
#[serde(default)]
pub admin_users: Vec<String>,
```

**鉴权**：`/approve`、`/deny`、`/permits` 三个命令在处理前校验 `msg.external_user_id ∈ admin_users`，不通过则回复 "permission denied"（不静默忽略——让误操作者知道为何无效）。注意这与 `check_access` 是两层：`check_access` 管"谁能跟 bot 说话"，`admin_users` 管"谁能审批"。命令在群聊与私聊中均可用（DM 兜底时审批动作即发生在私聊）。

### 5.6 FeishuAdapter 新增方法

```rust
impl FeishuAdapter {
    /// 为指定文档添加协作者（三类申请人批量授权）
    async fn grant_doc_permission(
        &self,
        file_token: &str,
        file_type: &str,
        req: &DocPermissionRequest,
        perm: &str,
    ) -> Result<(), ChannelError>;

    /// 私聊发送卡片（DM 兜底通知）：receive_id_type=open_id，
    /// 无需预先解析 p2p chat_id，API 隐式使用/创建单聊会话。
    async fn send_direct_card(&self, open_id: &str, card_json: &str) -> Result<Option<String>, ChannelError>;
}
```

`grant_doc_permission` 内部：构造 `members` 数组（user→`{member_type:"openid", type:"user"}`，chat→`"openchat"/"chat"`，department→`"opendepartmentid"/"department"`），调 `batch_create?type={file_type}&need_notification=true`，复用现有 `get_token` + `api_post` + `check_api_resp`。

`send_direct_card` 复用 `send_msg` 的 HTTP 路径，仅 `receive_id_type` 不同（现有 `RECEIVE_ID_TYPE` 常量硬编码为 `chat_id`，发送函数改为按目标类型传参）。hub 侧通过 `PlatformAdapter` trait 暴露的 DM 能力调用（默认实现返回 unsupported，仅 Feishu 落地）。

### 5.7 卡片按钮回调（card.action.trigger）

**接收**：ws 数据帧 header `type: card`；同时容忍以普通 `event` 帧（`event_type=card.action.trigger`）投递的形态。回调报文为 v2 信封 `{schema, header, event: {operator, action, context, token}}`（解析时先取 `event` 再回退顶层）。`handle_binary` 对所有数据帧**立即 ACK `{"code":200"}` 再解析**——不在 ACK 里携带 toast/卡片应答（响应格式未经实证，见 R8），终态反馈全部走后续的 `update_card` API 与消息，与命令路径完全一致。

**处理**：payload 包装为 `ChannelEvent::CardAction` 送入 incoming；hub 分支：

1. 从 `value` 解析 `{action, id}`，非法则忽略（warn 日志）；
2. 校验 `operator_open_id ∈ admin_users`，不通过则向回调上下文的会话发消息 "permission denied"（有 `chat_id` 时）；
3. 走与 `/approve`、`/deny` **完全相同**的 resolve 路径（条件更新抢单 → 授权 API → 回写全部通知卡片）——并发点击与命令混用都只生效一次，未抢到者在回调会话收到 "该申请已被处理"；
4. 成功但无任何通知卡片可回写时（`notify_msg_ids` 为空或全部更新失败），改为在回调会话发送结果消息，避免"无声审批"。

## 6. 数据流时序

```
申请人点击「申请权限」（飞书客户端）
  │
  ▼ 飞书服务端
ws 长连接 ──► type:event 帧, event_type=drive.file.permission_member_applied_v1
  │
  ▼ feishu.rs::parse_event_json
ChannelEvent::DocPermissionApplied ──► incoming
  │
  ▼ hub 处理循环
查重 → save_perm_request ──► id=3
投递通知卡片（approval_chat_id 管理群；缺省则 DM 兜底逐个通知 admin_users）
  ──► set_perm_notify_msgs(3, [msg_id...])
  │
  ▼ 管理员在管理群或私聊
"/approve 3 edit"
  │
  ▼ hub 命令分支
校验 external_user_id ∈ admin_users
resolve_perm_request(3, approved, admin, edit) ──► 抢到
grant_doc_permission(...) ──► batch_create(member openid ou_aaa, perm edit, need_notification)
update_card(全部 notify_msg_ids, "✅ #3 已批准 edit · by ou_admin")
回复 "已批准 #3：ou_aaa 获得 edit 权限"
  │
  ▼ 飞书系统
申请人收到「权限申请已通过」系统通知
```

## 7. 接口变化与影响范围

| 文件 | 变更 |
|------|------|
| `channels/mod.rs` | 新增 `ChannelEvent` 枚举、`DocPermissionRequest`、`CardAction`；`run_receiver` 签名改 `mpsc::Sender<ChannelEvent>`；`ChannelConfig` 加 `approval_chat_id`/`admin_users`；`ChannelStore` 加 5 个方法；`PlatformAdapter` 加 `send_direct_card`/`grant_doc_permission`（默认 unsupported） |
| `channels/approval.rs` | 通知卡片构建与投递（群 / DM 兜底）、三个审批命令的鉴权与处理、按钮回调处理、共享 resolve 路径（抢单 → 授权 → 回写全部卡片，失败回滚） |
| `channels/feishu.rs` | `parse_event_json` 改 `match` 分发；ws 数据帧 `card` 分支解析回调；新增 `grant_doc_permission`、`send_direct_card`；发送函数按目标类型传 `receive_id_type` |
| `channels/telegram.rs` | 发送侧包装 `ChannelEvent::Message`（适配签名） |
| `channels/hub.rs` | 处理循环分出事件/回调分支；`CMD_PREFIXES` 加 `/permits` `/approve` `/deny` 及解析，处理委托给 approval 模块 |
| `channels/store.rs` | 5 个方法的 SQLite 实现 |
| `storage/migrations.rs` | 新增 migration（`channel_doc_permission_requests`） |
| `docs/CONFIG.md` / `docs/config-schema.json` | 两个新配置字段 |

GC 关联：`channel_doc_permission_requests` 不随 session 回收（它是审批台账），`delete_by_sessions` 不涉及。

## 8. 实施计划

1. **Phase 1（接收 + 存储）**：`ChannelEvent` 改造（含 telegram 适配）、migration、`ChannelStore` 方法、`parse_event_json` 放行与解析。单测：事件 payload 解析（`feishu_test.rs`）、store CRUD（`store_test.rs`）。✅
2. **Phase 2（通知）**：`approval_chat_id`/`admin_users` 配置、通知卡片（含按钮）格式化与投递（管理群 / DM 兜底）、`notify_msg_ids` 记录。✅
3. **Phase 3（审批闭环）**：三个命令 + `admin_users` 鉴权 + `grant_doc_permission` + 并发抢单 + 通知卡片回写/结果回复。`hub_test.rs` 用 mock adapter 覆盖：正常批准、重复批准（未抢到）、非管理员拒绝、API 失败回滚。✅
4. **Phase 4（按钮回调）**：ws 数据帧 `card` 分支解析、`CardAction` 事件、hub 按钮审批分支（复用 Phase 3 resolve 路径）。单测：回调 payload 解析、非管理员拒绝、重复点击。✅

## 9. 风险与未决问题

| # | 风险 | 缓解 |
|---|------|------|
| R9 | 卡片按钮在 schema 2.0 中使用 v1 的 `action` 容器导致发送失败 | **已实测踩坑（API 200861）**：schema 2.0 移除了 `action` 容器，`button` 直接作为 body 元素 |
| R10 | 事件投递有分钟级延迟（实测 0~5 分钟），且长连接为集群模式随机单投 | 设计文档明确告知；调试期多连接（含被杀死的僵尸连接）会分走事件，生产单连接不受影响 |

| # | 风险 | 缓解 |
|---|------|------|
| R1 | 应用不是文档 owner/manager 时收不到事件 | 设计边界内：只覆盖应用自有文档；文档中明示该前提，避免误认为万能审批入口 |
| R2 | 事件重复推送（ws 重连后重投）导致重复落库 | `save_perm_request` 前以 `(file_token, 申请人集合, permission, status='pending')` 查重；已存在 pending 记录则跳过（飞书无申请唯一 id，此为最佳去重键） |
| R3 | 管理员误批 `full_access` | 命令要求显式参数才覆盖权限；通知卡片默认提示用申请值批准；`resolved_by`/`resolved_perm` 落库可审计 |
| R4 | `batch_create` 部分成员失败 | 响应含成功成员列表；回复中列出失败成员，已成功的不回滚（幂等，重发 `/approve` 可补齐——状态已是 approved，需提供 `/approve <id> --retry`？列为未决，MVP 手工处理） |
| R5 | 通知卡片发送成功但 `set_perm_notify_msgs` 失败 | 审批仍可闭环（仅失去卡片回写，退化为结果消息）；warn 日志 |
| R7 | DM 兜底时部分管理员的私聊卡片发送失败（不在可用范围等） | 逐人发送互不影响，只记录成功送达的 message_id；全部失败则仅落库 + 错误日志，管理员仍可用 `/permits` 查询审批 |
| R8 | `card` 数据帧的实际格式（header 取值、应答要求）未经实证 | 报文按 v2 信封解析（先取 `event` 再回退顶层），并容忍普通 `event` 帧投递；所有未知数据帧类型记 debug 日志，上线前先实测一轮；ACK 沿用 `{"code":200}` 通用应答，反馈全部走 `update_card` API + 消息，不依赖回调应答格式 |
| R6 | 事件里 `permission` 出现非枚举值（飞书未来扩展） | 解析时不做硬校验，落库原值；批准时校验——不在三枚举内则 reopen 并要求管理员用 `/approve <id> <perm>` 显式给级别（已实施） |

## 10. 未来项

- **文件级订阅自动化**：云文档事件需对每个文档调一次 subscribe API 才生成。若未来 yomi 增加文档创建能力（工具/lark-cli 包装），创建后应自动调用；或提供 `/watchdoc <token>` 通道命令让管理员手动补订阅。
- **回调应答 toast**：实证 `card` 帧应答格式后，在 ACK payload 携带 toast（"已批准 #3"），替代/补充消息反馈。
- `/revoke <id>`：`DELETE .../members/:member_id` 回收已授权限。
- 审批结果 DM 申请人（send message with `receive_id_type=open_id`）。
- 待审批提醒：cron 定时向管理群推送 pending 摘要。
