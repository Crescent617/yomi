# Channel require_mention 运行时覆盖（/mention 命令）

## 1. 背景

`require_mention`（群消息必须 @ 机器人才触发）目前只能写在频道配置里，改动要改配置 + 重启 daemon。实际使用中经常需要临时切换：调试期想让机器人在某群自由响应，或某群临时太吵想收紧为仅 @ 触发。

本设计新增 `/mention` 命令，在**会话容器维度**（thread 或 chat）运行时覆盖配置默认值，持久化、即时生效、可复位。

## 2. 语义

### 2.1 覆盖维度与回退链

容器沿用 history cursor 的同一概念（hub.rs `history_container`）：消息在 thread 内 → 容器为 thread（`omt_…`），否则为 chat（`oc_…`）。

gate 判定时的生效值（`effective_require_mention`）：

```
thread 消息：thread 覆盖 → chat 覆盖 → config.require_mention
chat 消息：  chat 覆盖 → config.require_mention
```

- 覆盖是**双向**的：配置 `true` 时可在某容器 `/mention off` 放开；配置 `false` 时也可 `/mention on` 收紧。
- DM（p2p）不适用：适配层恒置 `is_mention = true`，覆盖永远不会被查询。DM 内执行命令直接回复提示、不落库。
- 访问控制不变：`check_access`（allow/blocklist）在 mention 判定之前，覆盖只影响"是否需要 @"，绝不放宽准入。

### 2.2 与现有机制的交互（均保持不变）

- **history cursor**：`/mention` 是控制面命令，不加入 `consumes_history`——不推进 cursor；命令行本身也不进历史注入（`is_command_text` 既有逻辑）。
- **mid-run split / receipts**：命令不记 receipt，无交互。
- **ack reaction**：覆盖放开后，群里每条过 gate 的消息都会收到 ack reaction——这与配置级 `require_mention: false` 的既有行为一致（只是以前运行期够不着）。接受，不额外处理。
- **passive receipt**：覆盖放开后该容器不再有 `NotAddressed` 消息，每条消息都正常进 `handle_incoming_message`（即"机器人响应所有消息"——这正是关闭 mention 的语义）。

## 3. 命令面

```
/mention           查询：当前容器生效值 + 来源（thread 覆盖 / chat 覆盖 / 频道默认）
/mention on        本容器需要 @ 才触发（落覆盖）
/mention off       本容器无需 @ 即可触发（落覆盖）
/mention reset     清除本容器覆盖，回退到上级（thread→chat→配置）
```

- 解析：`parse_channel_command` 新增 `CMD_MENTION`；枚举新增 `Mention(Option<MentionMode>)`（`On` / `Off` / `Reset`）与 `InvalidMentionCommand`（用法提示）。
- 作用域：在 thread 内执行 → 写 thread 容器；在 chat 层执行 → 写 chat 容器。不提供跨容器参数（保持命令面小）。
- **权限**：变更（on/off/reset）限 `admin_users`（复用 approval.rs `check_admin`，同 `/restart`）——放开 mention 意味着机器人响应群里的每条消息，是费用/刷屏放大器，不能交给任意群成员。查询不限。
- ack 文案示例：
  - `本 thread 已关闭 @ 要求（覆盖频道默认：开启）。`
  - `已清除本 chat 的覆盖，回退到频道默认：开启。`
  - 查询：`当前 thread：需要 @（chat 覆盖）；频道默认：开启。`
- `HELP_TEXT` 增加一行。

## 4. 存储

新表（迁移追加到 `storage/migrations.rs`，命名 `add_channel_mention_overrides`）：

```sql
CREATE TABLE channel_mention_overrides (
    channel_name TEXT NOT NULL,
    container_id TEXT NOT NULL,   -- thread_id 或 chat_id
    require_mention INTEGER NOT NULL,  -- 0/1
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (channel_name, container_id)
)
```

`ChannelStore` trait 新增（默认空实现，SqliteChannelStore 实现，镜像 history cursor 的写法）：

```rust
async fn get_mention_override(&self, channel_name: &str, container_id: &str)
    -> KernelResult<Option<bool>>;
async fn set_mention_override(&self, channel_name: &str, container_id: &str,
    require_mention: bool) -> KernelResult<()>;
async fn clear_mention_override(&self, channel_name: &str, container_id: &str)
    -> KernelResult<()>;
```

GC：覆盖行极小；thread 容器覆盖随 thread 消亡成为死行，量可忽略，不做清理（与 history cursor 同策略）。

## 5. gate 改动

`gate_message` 签名加 `store: &Arc<dyn ChannelStore>`（处理循环处本就有）：

```rust
let require = effective_require_mention(store, config, msg).await;
let addressed = !require || msg.is_mention;
```

`effective_require_mention`：群消息按 §2.1 回退链查询（thread 消息最多两次点查，chat 一次）；DM 直接返回 `config.require_mention`（不查库）。sqlite 本地点查，每群消息一次，量与现有 cursor 读写同阶，不加缓存。

## 6. 变更点

| 文件 | 改动 |
|------|------|
| `storage/migrations.rs` | 新增 `add_channel_mention_overrides` |
| `channels/mod.rs` | `ChannelStore` trait 三个方法（默认实现） |
| `channels/store.rs` | SqliteChannelStore 实现 |
| `channels/hub.rs` | `ChannelCommand::Mention` / `InvalidMentionCommand`；`parse_channel_command`；`handle_incoming_message` 新分支（admin 校验 + 查询/变更文案）；`gate_message` 加 store 参数 + `effective_require_mention`；`HELP_TEXT` |

## 7. 测试

- override off：非 @ 群消息过 gate（allow）；override on（配置 false 时）：非 @ 消息回到 `NotAddressed`；
- thread 覆盖优先于 chat 覆盖；无 thread 覆盖时回退 chat；两级都无回退配置；
- reset 后回到上级/配置默认；
- 非 admin 执行 on/off/reset 被拒，查询放行；
- `/mention` 不推进 history cursor（镜像 `refused_thread_command_does_not_advance_history_cursor` 的写法）；
- DM 内执行返回提示且不落库；
- store 层：set/get/clear round-trip（镜像 `test_history_cursor_round_trip`）。
