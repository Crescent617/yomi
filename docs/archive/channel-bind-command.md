# 设计文档：通道 `/bind` 命令 —— 会话绑定到当前话题

**Status:** Implemented（2026-08-06，语义经确认：同 chat 共享 + 收养无路由会话，管理员权限；E2E 通过——新话题 `/bind` 后续聊暗号正确）
**Date:** 2026-08-06

---

## 1. 背景与问题

会话与话题的绑定目前完全由系统自动决定（chat 级 / thread 根 / 评论组各一个 session）。合法需求存在：

- 把 **GUI/CLI 里聊了一半的会话**接到飞书话题里继续（这类会话没有通道路由）；
- 同一群内的另一个话题想**续用**某话题积累的上下文；
- 文档评论组想绑到既有会话。

通道命令面已有 `/info` `/models` 等，缺一个"改绑定"的出口。

## 2. 设计

### 2.1 命令

```
/bind                → 回显当前 scope 绑定的 session id（只读）
/bind <session_id>   → 把当前 scope 的映射改指到该 session（管理员）
```

scope = 消息所属的 mapping key（thread 消息→话题根；群顶层→chat_id；评论组→`doc:…` key；RIT 顶层消息→其自身 id，配合回复锚定实现"开新话题续跑该会话"）。

### 2.2 语义（已确认）

- **同 chat 共享**：目标 session 已路由到**本 channel 本 chat** → 允许绑定（多话题共用上下文正是目的）；回复路由的不确定性在同 chat 内无害。
- **跨 chat 拒绝**：目标 session 已路由到**别的 chat/channel** → 拒绝（回复可能送错群，隐私问题）。
- **收养**：目标 session **无通道路由**（GUI/CLI 创建）→ 允许绑定，路由指向当前 chat/评论组。
- **幂等提示**：目标就是当前绑定 → 直接告知，不写库。

### 2.3 权限

仅 `admin_users`（复用 `approval::check_admin`，与 `/mention` 变更、`/approve` 同级）——绑定改写回复路由，属敏感操作。

### 2.4 实现要点

- `ChannelCommand::Bind(Option<String>)` + `CMD_PREFIXES` 加 `/bind`；
- 绑定即 `store.save_mapping(channel, mapping_key, sid, actual_chat_id, reply_anchor)`——不写 session 本身，不创建新 session（目标不存在 → 报错）；
- 存在性校验 `kernel.get_session`；路由校验 `store.find_routing_by_session`；
- 确认回复走 `send_command_reply`（评论组自动落评论串）；
- `/bind` 无参数时回显当前 scope 的 session id（无则提示尚未产生）。

## 3. 影响范围

| 文件 | 变更 |
|------|------|
| `channels/hub.rs` | `ChannelCommand::Bind` + 解析 + 处理分支 + HELP 文本 |
| `hub_test.rs` | 回显/绑定/不存在/跨 chat 拒绝/非管理员/幂等用例 |
| `docs/CONFIG.md` | 命令说明（如表格列命令则补一行） |

无 schema/配置变更。

## 4. 风险

| # | 风险 | 缓解 |
|---|------|------|
| R1 | 同 chat 共享时回复锚点（reply_msg_id）来自最后一次映射写入 | 每次绑定/触发本就刷新锚点，影响限于话题内定位，可接受 |
| R2 | 误绑他人 session 造成上下文串味 | 管理员权限 + 确认回复里回显目标 session 标题/id，误操作可见可逆（再 `/bind` 回去或 `/clear`） |
