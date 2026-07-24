# 设计文档：Feishu 云文档权限申请受理 —— 事件接收 + 管理群命令审批

**Status:** Draft
**Date:** 2026-07-23

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
- 格式化为卡片通知到**指定管理群**；
- 管理员在管理群用命令批准（可改权限级别）/ 拒绝；
- 批准后申请人收到飞书系统通知；审批结果回写通知卡片；
- 并发审批只生效一次，全程有审计（谁、何时、批了什么）。

**非目标**

- **不做卡片交互按钮**（同意/拒绝 button）。`card.action.trigger` 是「回调」而非「事件」：ws 协议以 `type: card` 数据帧下发，但官方 SDK（python 1.7.1 / node 1.71.1 / go v3）的 ws 客户端均直接丢弃该帧，官方支持路径是 HTTP webhook（回调订阅的请求地址）。yomi 当前是纯长连接、无 HTTP 入站，引入 webhook 属另一项工程（见 §10 未来项）。
- 不做审批流（飞书 Approval 应用对接）。
- 不主动 DM 申请人告知结果（批准由飞书系统 `need_notification` 通知；拒绝不通知，与飞书原生行为一致）。
- 不处理"机器人自身的工具执行权限"（kernel `AgentEvent::PermissionRequest`）——那是另一条链路，channel session 目前是 `auto_approve_level: Dangerous`，与本设计无关。

## 3. 前置条件（运维侧）

| 项 | 说明 |
|----|------|
| 事件订阅 | 开发者后台 → 事件订阅 → 添加 `drive.file.permission_member_applied_v1`（文件协作者权限申请）。长连接方式即可收到（普通事件，`type: event` 帧） |
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
/// 平台入站载荷：聊天消息 或 平台事件
#[derive(Debug, Clone)]
pub enum ChannelEvent {
    Message(ChannelMessage),
    DocPermissionApplied(DocPermissionRequest),
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
```

- `PlatformAdapter::run_receiver` 签名改为 `mpsc::Sender<ChannelEvent>`；Telegram 只构造 `Message` 变体，改动最小。
- `feishu.rs::parse_event_json` 的 `event_type` 匹配从单一放行改为 `match`：`im.message.receive_v1` 走现有逻辑；`drive.file.permission_member_applied_v1` 解析后包装为 `ChannelEvent::DocPermissionApplied`；其余仍忽略。
- hub 处理循环 `match` 两个分支：`Message` 走现有逻辑；`DocPermissionApplied` 走 §5.3 通知逻辑。**平台事件不做 `check_access`/`require_mention` 过滤**（它不是用户消息）。

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
    notify_msg_id TEXT,                            -- 通知卡片 message_id（回写用）
    resolved_by TEXT,                              -- 审批人 open_id
    resolved_perm TEXT,                            -- 实际授予的权限
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    resolved_at TIMESTAMP
);
```

`ChannelStore` trait 增加：

```rust
async fn save_perm_request(&self, channel: &str, req: &DocPermissionRequest) -> KernelResult<i64>;
async fn set_perm_notify_msg(&self, id: i64, msg_id: &str) -> KernelResult<()>;
async fn list_pending_perm_requests(&self, channel: &str) -> KernelResult<Vec<PermRequestRow>>;
/// 条件更新：仅当 status='pending' 时翻转，返回是否抢到（并发审批只赢一次）
async fn resolve_perm_request(
    &self, id: i64, status: &str, resolved_by: &str, resolved_perm: Option<&str>,
) -> KernelResult<Option<PermRequestRow>>;
```

短 id（自增整数）即管理员命令引用的编号。过期不做主动清理：`/permits` 列表展示创建时间，由人判断（飞书侧申请长期不处理自动失效，与本地状态无一致性强依赖）。

### 5.3 通知：管理群卡片

hub 收到 `DocPermissionApplied`：

1. `save_perm_request` 落库，拿到 `id`；
2. 格式化卡片（schema 2.0，橙 header）发送到 **`approval_chat_id`**（新增配置，见 §5.5）：

```
┌ 📄 文档权限申请 #3 ─────────────────────┐
│ 申请人    ou_aaa（等 1 人）· 群 oc_bbb     │
│ 文档      docx/doxcnXXXX（链接可点）        │
│ 申请权限  view                             │
│ 备注      求权限看下方案                     │
│ ─────────────────────────────           │
│ 批准: /approve 3 [view|edit|full_access]  │
│ 拒绝: /deny 3                            │
└──────────────────────────────────────────┘
```

3. 发送成功后 `set_perm_notify_msg` 记录 `notify_msg_id`，供审批后回写。

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
update_card(notify_msg_id, 终态卡片)  // 绿："✅ #3 已批准 view · by ou_admin"
+ 回复确认消息（线程内）
```

`/deny`：`resolve_perm_request(id, denied, ...)` 成功即回写通知卡片（灰："❌ #3 已拒绝 · by ou_admin"），不调任何飞书 API。

**通知卡片回写**复用可观测性设计的 `PlatformAdapter::update_card`（见 `feishu-channel-observability.md`）；若该能力未落地，退化为追加一条结果消息，不阻塞本设计。

### 5.5 配置

`ChannelConfig` 增加（`snake_case`，同步 `docs/config-schema.json` 与 `docs/CONFIG.md`）：

```rust
/// 文档权限申请通知目标群 chat_id；缺省则该功能关闭（收到事件仅记日志）
#[serde(default)]
pub approval_chat_id: Option<String>,
/// 有审批权的管理员 open_id 列表；缺省则任何人不得审批
#[serde(default)]
pub admin_users: Vec<String>,
```

**鉴权**：`/approve`、`/deny`、`/permits` 三个命令在处理前校验 `msg.external_user_id ∈ admin_users`，不通过则回复 "permission denied"（不静默忽略——让误操作者知道为何无效）。注意这与 `check_access` 是两层：`check_access` 管"谁能跟 bot 说话"，`admin_users` 管"谁能审批"。

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
}
```

内部：构造 `members` 数组（user→`{member_type:"openid", type:"user"}`，chat→`"openchat"/"chat"`，department→`"opendepartmentid"/"department"`），调 `batch_create?type={file_type}&need_notification=true`，复用现有 `get_token` + `api_post` + `check_api_resp`。

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
save_perm_request ──► id=3
send_card(approval_chat_id, 通知卡片) ──► set_perm_notify_msg(3, msg_id)
  │
  ▼ 管理员在管理群
"/approve 3 edit"
  │
  ▼ hub 命令分支
校验 external_user_id ∈ admin_users
resolve_perm_request(3, approved, admin, edit) ──► 抢到
grant_doc_permission(...) ──► batch_create(member openid ou_aaa, perm edit, need_notification)
update_card(notify_msg_id, "✅ #3 已批准 edit · by ou_admin")
回复 "已批准 #3：ou_aaa 获得 edit 权限"
  │
  ▼ 飞书系统
申请人收到「权限申请已通过」系统通知
```

## 7. 接口变化与影响范围

| 文件 | 变更 |
|------|------|
| `channels/mod.rs` | 新增 `ChannelEvent` 枚举、`DocPermissionRequest`；`run_receiver` 签名改 `mpsc::Sender<ChannelEvent>`；`ChannelConfig` 加 `approval_chat_id`/`admin_users`；`ChannelStore` 加 4 个方法 |
| `channels/feishu.rs` | `parse_event_json` 改 `match` 分发；新增 `grant_doc_permission` |
| `channels/telegram.rs` | 发送侧包装 `ChannelEvent::Message`（适配签名） |
| `channels/hub.rs` | 处理循环分支出事件分支；`CMD_PREFIXES` 加 `/permits` `/approve` `/deny` 及解析；审批流程；`admin_users` 鉴权 |
| `channels/store.rs` | 4 个方法的 SQLite 实现 |
| `storage/migrations.rs` | 新增 migration（`channel_doc_permission_requests`） |
| `docs/CONFIG.md` / `docs/config-schema.json` | 两个新配置字段 |

GC 关联：`channel_doc_permission_requests` 不随 session 回收（它是审批台账），`delete_by_sessions` 不涉及。

## 8. 实施计划

1. **Phase 1（接收 + 存储）**：`ChannelEvent` 改造（含 telegram 适配）、migration、`ChannelStore` 方法、`parse_event_json` 放行与解析。单测：事件 payload 解析（`feishu_test.rs`）、store CRUD（`store_test.rs`）。
2. **Phase 2（通知）**：`approval_chat_id` 配置、通知卡片格式化与发送、`notify_msg_id` 记录。
3. **Phase 3（审批闭环）**：三个命令 + `admin_users` 鉴权 + `grant_doc_permission` + 并发抢单 + 通知卡片回写/结果回复。`hub_test.rs` 用 mock adapter 覆盖：正常批准、重复批准（未抢到）、非管理员拒绝、API 失败回滚。

## 9. 风险与未决问题

| # | 风险 | 缓解 |
|---|------|------|
| R1 | 应用不是文档 owner/manager 时收不到事件 | 设计边界内：只覆盖应用自有文档；文档中明示该前提，避免误认为万能审批入口 |
| R2 | 事件重复推送（ws 重连后重投）导致重复落库 | `save_perm_request` 前以 `(file_token, 申请人集合, permission, status='pending')` 查重；已存在 pending 记录则跳过（飞书无申请唯一 id，此为最佳去重键） |
| R3 | 管理员误批 `full_access` | 命令要求显式参数才覆盖权限；通知卡片默认提示用申请值批准；`resolved_by`/`resolved_perm` 落库可审计 |
| R4 | `batch_create` 部分成员失败 | 响应含成功成员列表；回复中列出失败成员，已成功的不回滚（幂等，重发 `/approve` 可补齐——状态已是 approved，需提供 `/approve <id> --retry`？列为未决，MVP 手工处理） |
| R5 | 通知卡片发送成功但 `set_perm_notify_msg` 失败 | 审批仍可闭环（仅失去卡片回写，退化为结果消息）；warn 日志 |
| R6 | 事件里 `permission` 出现非枚举值（飞书未来扩展） | 解析时不做硬校验，落库原值；批准时若不在三枚举内则要求管理员显式给 perm |

## 10. 未来项

- **卡片按钮审批**：引入 HTTP 回调 endpoint（可挂 `crates/kernel/src/server`），订阅 `card.action.trigger`，按钮 value 携带申请 id，回调内走与 `/approve` 相同的 `resolve_perm_request` + 授权路径，同步返回更新后卡片。届时命令与按钮并存。
- `/revoke <id>`：`DELETE .../members/:member_id` 回收已授权限。
- 审批结果 DM 申请人（send message with `receive_id_type=open_id`）。
- 待审批提醒：cron 定时向管理群推送 pending 摘要。
