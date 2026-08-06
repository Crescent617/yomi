# 设计文档：通道事件开关（字符串数组）—— 以文档评论为首个成员

**Status:** Implemented（2026-08-06，黑名单语义经确认并落地，E2E 通过）
**Date:** 2026-08-06

---

## 1. 背景与问题

飞书文档评论功能（`feishu-doc-comment.md`）当前**无运行时开关**：只要控制台订阅了 `drive.notice.comment_add_v1`，事件到达即触发 agent。想临时停用它只能去开发者后台退订/下版本——代价大、还有发布审核延迟。

设计约束：

- 开关机制要**通用**：通道会接入更多平台事件类功能（评论、未来的表情/成员事件等）；
- **平台异构**：各平台的事件词表不同（`doc_comment` 是飞书专属概念），typed struct 会把平台专属字段挂到所有平台的配置上——因此用**字符串数组**，词表归各平台所有。

## 2. 目标与非目标

**目标**

- 给文档评论功能加运行时开关；
- 机制通用且平台无关：`ChannelConfig` 只承载一个字符串数组，合法取值由各平台自行定义。

**非目标**

- 不改动审批功能的既有门控方式（`approval_chat_id`/`admin_users` 存在即启用），迁入事件词表留作未来项；
- 不做热更新（沿用配置加载既有语义）。

## 3. 配置设计

```rust
/// 停用的平台事件功能（词表按平台定义，见 `known_event_names`）；
/// 缺省 = 全部启用。事件推送本身需控制台订阅，此开关用于运行期临时停用。
#[serde(default)]
pub disabled_events: Vec<String>,
```

```toml
[[channels]]
name = "feishu"
disabled_events = ["doc_comment"]   # 临时关闭文档评论触发
```

**取黑名单（disabled）而非白名单（enabled）**，理由：

- 目标用例是「临时关掉某功能」——黑名单写法直白（`disabled_events = ["doc_comment"]`）；白名单要表达「关掉唯一的功能」得写 `events = []`，语义反直觉，且未来新增功能会静默改变 `[]` 的含义；
- 与代码库既有气质一致：功能默认开、靠配置关（`blocked_users`/`blocked_chats` 同为 adjective-first 黑名单命名）；
- 白名单模式下「缺省 = 全开 / 出现 = 仅列出的开」的 Option 语义是经典 footgun。

**词表管理**：事件名集中在 `mod.rs` 定义常量（`EVENT_DOC_COMMENT: &str = "doc_comment"`），`PlatformConfig::known_event_names()` 按平台返回合法集合。启动时校验：数组里的未知名字仅 **warn 日志**提示（不拒绝——旧二进制遇到新名字不该起不来；schema 侧另由编辑器校验兜底）。

## 4. 生效点

`channels/comment.rs::handle_doc_comment_added` 策略链**第 0 步**：

```rust
if config.disabled_events.iter().any(|e| e == EVENT_DOC_COMMENT) {
    debug!(channel = %channel_name, "doc comment ignored (feature disabled)");
    return;
}
```

选在 handler 而非 gate 循环分支：可单测（gate 循环是 spawn 的机械结构；此处与 notice_type / is_mentioned / 名单过滤同处一链）；关闭时零代价——不拉评论、不建 session，仅一行 debug 日志。adapter 层无感知（协议层本就无 config 可见性）。

## 5. 影响范围

| 文件 | 变更 |
|------|------|
| `channels/mod.rs` | `ChannelConfig.disabled_events` + `EVENT_DOC_COMMENT` 常量 + `PlatformConfig::known_event_names()` |
| `channels/comment.rs` | 策略链第 0 步检查 |
| `channels/hub.rs` | `start_instance` 启动时对未知事件名 warn |
| `channels/comment_test.rs` | 关闭用例：无 dispatch 且 mock 的 `fetch_doc_comment` 零调用（证明零 API 代价） |
| `docs/CONFIG.md` | channels 配置表加一行 |
| `docs/config-schema.json` | 加 `disabled_events` 数组（`additionalProperties: false` 下必须加） |

## 6. 实施计划

单步完成：常量与配置 → 检查 → 启动校验 → 测试 → 文档同步。

## 7. 风险

| # | 风险 | 缓解 |
|---|------|------|
| R1 | 字符串拼写错误（`doc_comments`）静默无效 | schema `items.enum` 编辑器校验 + 启动 warn 双保险（serde 对数组内容无法拒绝未知值） |
| R2 | 未来某事件功能需要「默认关、显式开」 | 黑名单表达不了默认关——此类功能沿用审批式的「配置存在即开」显式门控，不进 disabled_events 词表 |

## 8. 未来项

- `doc_permission` 入词表（审批功能的运行时开关，需先定义与隐式门控的优先级）；
- 各平台新事件功能直接扩 `known_event_names()`。
