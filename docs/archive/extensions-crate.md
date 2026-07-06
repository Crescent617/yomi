# Extensions Crate 拆分设计文档

**状态**: 草案 (Draft)  
**作者**: Yomi  
**日期**: 2026-06-29  
**关联 Issue**: 将 kernel 的 channels 独立到 extensions crate

---

## 1. 背景与目标

### 1.1 当前问题

`kernel` crate 包含了 `channels` 子系统（飞书、Telegram 等平台适配），这导致：

- **依赖膨胀**：kernel 必须依赖 `teloxide-core`、`tokio-tungstenite`、`lark-websocket-protobuf`、`mime_guess` 等平台专用库，即使不需要 channels 的应用（如纯本地 CLI）也全部拉取。
- **编译时间增加**：channels 涉及 WebSocket、HTTP 客户端、Protobuf 解码等重型代码，拖慢 kernel 的编译。
- **模块边界模糊**：核心 runtime（agent、coordinator、storage）与外部平台集成耦合在一起，不符合"单一职责"原则。
- **扩展性受限**：新增一个平台适配器需要修改 kernel 源码，破坏了第三方扩展的能力。

### 1.2 目标

- 在 **kernel** 中定义扩展的标准接口（`Extension` trait），使其对任何扩展平台实现一无所知。
- 在 **新的 `extensions` crate** 中实现 `channels` 扩展（飞书、Telegram）。
- 保持 `kernel` 的 API 稳定性，上层的调用代码修改量最小化。
- 为将来支持更多扩展平台（如微信、Discord 等）建立清晰的架构框架。

---

## 2. 设计概述

### 2.1 核心原则

| 原则 | 说明 |
|------|------|
| **Kernel 定义接口** | kernel 只包含 trait 和共享类型，不含任何平台实现 |
| **Extensions 实现接口** | extensions crate 依赖 kernel，实现 `Extension` trait |
| **无循环依赖** | kernel 不引用 extensions 中的任何类型 |
| **依赖注入** | 具体扩展通过 `build_coordinator` / `init_coordinator` 的参数注入 |
| **Kernel 做路由** | 扩展需要调用 kernel 的会话系统时，通过 `MessageRouter` trait 间接访问 |

### 2.2 目标架构

```
┌─────────────────────────────────────────────────────────────────┐
│                          Applications                           │
│  (cli / gui / tui)                                              │
│  ── 构造 `extensions::ChannelsExtension`                        │
│  ── 调用 `kernel::build_coordinator(config, extension)`         │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                      extensions crate                           │
│  ── `ChannelsExtension` (impl `kernel::Extension`)              │
│  ── `ChannelHub` (生命周期管理)                                 │
│  ── `TelegramAdapter` / `FeishuAdapter`                         │
│  依赖: teloxide-core, tokio-tungstenite, lark-websocket-protobuf│
└─────────────────────────────────────────────────────────────────┘
                              │  depends on
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                        kernel crate                             │
│  ── `Extension` trait                                          │
│  ── `MessageRouter` trait                                        │
│  ── `channels` 模块 (types + traits + ChannelError)             │
│  ── `Kernel` (impl `MessageRouter`)                         │
│  ── `StorageSet` (含 `SqliteChannelStore`)                     │
│  ── 不含任何平台专用库                                           │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. 核心 trait 设计

### 3.1 `Extension` trait

定义在 `kernel/src/extension.rs`。

扩展是 kernel 的可选外挂能力。每个扩展实现一个独立的长期运行任务（如接收消息），并可通过 `Extension` 的 API 查询状态。

```rust
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::channels::{ChannelInfo, PlatformAdapter};
use crate::types::{Result, SessionId};

/// 扩展的标准接口。
/// 每个扩展实例由应用层创建，注入到 `Kernel` 中。
#[async_trait]
pub trait Extension: Send + Sync {
    /// 扩展名称，用于日志和调试。
    fn name(&self) -> &str;

    /// 启动扩展。
    /// 接收 `MessageRouter` 用于访问 kernel 的会话系统。
    /// 接收 `CancellationToken` 用于优雅退出。
    async fn start(
        &self,
        router: Arc<dyn MessageRouter>,
        cancel: CancellationToken,
    ) -> Result<()>;

    /// 关闭扩展，等待所有内部任务退出。
    async fn shutdown(&self) -> Result<()>;

    /// 返回当前运行的 channel 状态列表。
    fn channels(&self) -> Vec<ChannelInfo>;

    /// 根据 session ID 查找对应的平台适配器（用于 send_message 工具等）。
    async fn get_adapter_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<(String, Arc<dyn PlatformAdapter>)>>;
}
```

### 3.2 `MessageRouter` trait

定义在 `kernel/src/extension.rs`。这是 kernel 暴露给扩展的**单向窗口**：扩展可以调用 kernel 的会话管理 API，但不需要了解 `Kernel` 的全貌。

```rust
use crate::app::coordinator::CreateSessionInput;
use crate::event::Event;
use crate::types::{ContentBlock, Result, SessionId};
use tokio::sync::{broadcast, RwLock};
use std::sync::Arc;

/// 会话管理接口，由 `Kernel` 实现。
/// 扩展通过此 trait 与 kernel 的会话系统交互，无需依赖 `Kernel` 本身。
#[async_trait]
pub trait MessageRouter: Send + Sync {
    /// 创建新会话，返回 session ID。
    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId>;

    /// 从存储中恢复会话，返回 session ID。
    async fn restore_session(
        &self,
        session_id: &SessionId,
        tool_blocklist: Vec<String>,
    ) -> Result<SessionId>;

    /// 获取内存中的会话（用于检查是否已加载）。
    fn get_session(&self, session_id: &SessionId) -> Option<Arc<RwLock<crate::app::Session>>>;

    /// 向会话发送消息内容。
    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()>;

    /// 订阅会话事件流（用于回传消息到外部平台）。
    fn subscribe_session_events(&self, session_id: &SessionId) -> Option<broadcast::Receiver<Event>>;
}
```

#### 为什么不用 `Kernel` 直接当接口？

- `Kernel` 包含大量内核内部方法（cron、project、storage 等），如果扩展持有 `Kernel` 的强引用，会破坏抽象边界。
- `MessageRouter` 只暴露扩展需要的**最小接口**，未来添加新扩展平台时无需修改 `Kernel` 的签名。
- 避免循环依赖：`extensions` 依赖 `kernel` 的 `Extension` 和 `MessageRouter` trait，`kernel` 不依赖 `extensions`。

### 3.3 `Kernel` 实现 `MessageRouter`

```rust
#[async_trait]
impl MessageRouter for Kernel {
    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        self.create_session(input).await
    }

    async fn restore_session(&self, session_id: &SessionId, tool_blocklist: Vec<String>) -> Result<SessionId> {
        self.restore_session(session_id, tool_blocklist).await
    }

    fn get_session(&self, session_id: &SessionId) -> Option<Arc<RwLock<Session>>> {
        self.get_session(session_id)
    }

    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        self.send_message(session_id, blocks).await
    }

    fn subscribe_session_events(&self, session_id: &SessionId) -> Option<broadcast::Receiver<Event>> {
        self.subscribe_session_events(session_id)
    }
}
```

---

## 4. 模块拆分详细方案

### 4.1 kernel 保留的内容（精简版 channels）

文件位置：`kernel/src/channels/mod.rs`

保留所有**类型、trait、辅助函数**，移除所有**平台实现、ChannelHub 逻辑**。

保留内容清单：
- `ChannelError`（错误类型）
- `ChannelStatus` / `ChannelConfig` / `PlatformConfig` / `ChannelMessage` / `ChannelInfo`（配置与数据类型）
- `ChannelStore`（存储 trait）
- `PlatformAdapter`（平台适配器 trait）
- `blocks_to_text`（通用工具函数）
- `ChannelConfig::check_access`（访问控制逻辑）

移除内容：
- `ChannelHub` 结构体及其实现 → 移到 `extensions/src/channels/hub.rs`
- `TelegramAdapter` → 移到 `extensions/src/channels/telegram.rs`
- `FeishuAdapter` → 移到 `extensions/src/channels/feishu.rs`
- `build_adapter` 函数 → 移到 `extensions` 的 `ChannelsExtension` 构造逻辑中

### 4.2 kernel 新增的模块

| 文件 | 说明 |
|------|------|
| `kernel/src/extension.rs` | `Extension` + `MessageRouter` trait |
| `kernel/src/storage/channel_store.rs` | 将 `SqliteChannelStore` 从 `channels/store.rs` 移入 storage 体系 |

### 4.3 kernel 清理的模块

| 文件 | 操作 |
|------|------|
| `kernel/src/channels/hub.rs` | 删除（移至 extensions） |
| `kernel/src/channels/telegram.rs` | 删除（移至 extensions） |
| `kernel/src/channels/feishu.rs` | 删除（移至 extensions） |
| `kernel/src/channels/store.rs` | 删除（并入 `storage/channel_store.rs`） |
| `kernel/src/channels/utils.rs` | 删除（`resolve_safe_path` 移入 `utils/path.rs`） |
| `kernel/src/channels/mod.rs` | 保留类型 + trait，删除实现 |

### 4.4 extensions 的新模块

```
extensions/
├── Cargo.toml
├── src/
│   ├── lib.rs                  # 暴露 `ChannelsExtension`
│   ├── channels/
│   │   ├── mod.rs              # 内部模块组织 + `MAX_RETRY_DELAY`
│   │   ├── hub.rs              # `ChannelHub` + 实现 `Extension` trait
│   │   ├── telegram.rs         # `TelegramAdapter` (impl `PlatformAdapter`)
│   │   ├── feishu.rs           # `FeishuAdapter` (impl `PlatformAdapter`)
│   │   └── utils.rs            # 扩展专用工具（如 `resolve_safe_path` 的副本，或直接用 kernel 的）
│   └── ...                     # 未来可扩展其他类型（如 webhooks, slack 等）
```

---

## 5. 依赖关系

### 5.1 kernel 移除的依赖

以下依赖从 `kernel/Cargo.toml` 中移除，因为它们只在 channels 实现中使用：

- `teloxide-core`
- `tokio-tungstenite`（保留 `connect` / `rustls-tls-native-roots` feature）
- `lark-websocket-protobuf`
- `mime_guess`
- `prost`

> 注意：`reqwest` 在 kernel 中仍有其他用途（provider 的 HTTP 请求），保留。

### 5.2 extensions 的依赖

```toml
[dependencies]
kernel = { path = "../kernel", package = "kernel" }
tokio = { workspace = true }
async-trait = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
tokio-util = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
base64 = { workspace = true }
futures = { workspace = true }
dashmap = { workspace = true }

# channels 专用依赖
teloxide-core = { workspace = true }
tokio-tungstenite = { workspace = true, features = ["connect", "rustls-tls-native-roots"] }
lark-websocket-protobuf = "0.1.1"
mime_guess = { workspace = true }
prost = "0.13"
```

### 5.3 依赖方向图

```
extensions ──depends on──► kernel
    │
    └── teloxide-core, tokio-tungstenite, lark-websocket-protobuf, ...

gui ──depends on──► kernel
   │
   └── also depends on──► extensions (for daemon mode)

cli ──depends on──► kernel
   │
   └── (optional) depends on──► extensions (for daemon mode)
```

---

## 6. API 变更

### 6.1 `build_coordinator` 签名变更

**变更前：**
```rust
pub async fn build_coordinator(
    config: &Config,
    enable_cron: bool,
    start_channels: bool,
) -> Result<Arc<Kernel>>;
```

**变更后：**
```rust
pub async fn build_coordinator(
    config: &Config,
    enable_cron: bool,
    extension: Option<Arc<dyn Extension>>,
) -> Result<Arc<Kernel>>;
```

**变更原因：**
- `start_channels: bool` 的语义被扩展的注入所取代。应用层负责创建并传入扩展实例。
- kernel 本身不再关心 channels 是否启动，只负责将扩展注册到 `Kernel` 中。

### 6.2 `init_coordinator` 签名变更

**变更前：**
```rust
pub async fn init_coordinator(
    config_path: Option<&PathBuf>,
    enable_cron: bool,
    start_channels: bool,
) -> Result<(Arc<Kernel>, Config, Option<PathBuf>)>;
```

**变更后：**
```rust
pub async fn init_coordinator(
    config_path: Option<&PathBuf>,
    enable_cron: bool,
    extension: Option<Arc<dyn Extension>>,
) -> Result<(Arc<Kernel>, Config, Option<PathBuf>)>;
```

### 6.3 `Kernel` 结构体字段变更

**变更前：**
```rust
pub struct Kernel {
    // ...
    pub(crate) channel_manager: Option<Arc<crate::channels::hub::ChannelHub>>,
}
```

**变更后：**
```rust
pub struct Kernel {
    // ...
    pub(crate) extension: Option<Arc<dyn Extension>>,
}
```

相关方法：
- `list_channels()` → 委托给 `extension.channels()`
- `channel_manager()` → 改为 `extension()`
- `shutdown_extensions()` → 新增方法，委托 `extension.shutdown()`

### 6.4 `AgentShared` 字段变更

**变更前：**
```rust
pub struct AgentShared {
    // ...
    pub channel_hub: Option<Arc<crate::channels::hub::ChannelHub>>,
}
```

**变更后：**
```rust
pub struct AgentShared {
    // ...
    pub extension: Option<Arc<dyn Extension>>,
}
```

### 6.5 `SendMessageTool` 恢复（可选）

当前 `factory.rs` 中 `SendMessageTool` 的注册被注释掉了。本次重构为恢复它扫清了障碍：

```rust
pub struct SendMessageTool {
    extension: Arc<dyn Extension>,
}

impl SendMessageTool {
    pub fn new(extension: Arc<dyn Extension>) -> Self { ... }
}

// 在 Tool::exec 中
let (chat_id, adapter) = self.extension
    .get_adapter_for_session(&session_id)
    .await?
    .ok_or("not connected to external platform")?;
```

恢复与否由后续 PR 决定，本次重构只确保架构支持。

---

## 7. 上层调用代码变更

需要修改三个入口点：

### 7.1 `gui/src/daemon.rs` (line 94)

```rust
// 变更前
let (coordinator, _config, _config_file) = kernel::init_coordinator(None, true, true).await?;

// 变更后
let extension = if !config.channels.is_empty() {
    Some(Arc::new(extensions::ChannelsExtension::new(
        config.channels.clone(),
        storage.channel_store(),
        cancel.clone(),
    )) as Arc<dyn kernel::Extension>)
} else { None };
let (coordinator, _config, _config_file) = kernel::init_coordinator(None, true, extension).await?;
if let Some(ref ext) = coordinator.extension() {
    ext.start(coordinator.clone(), cancel).await?;
}
```

### 7.2 `cli/src/commands/daemon.rs` (line 52)

```rust
// 变更前
let (coordinator, config, _) = kernel::init_coordinator(None, true, true).await?;

// 变更后
let (coordinator, config, _) = kernel::init_coordinator(None, true, extension).await?;
```

### 7.3 `cli/src/commands/tui.rs` (line 244)

```rust
// 变更前
let coordinator = kernel::build_coordinator(config, false, false).await?;

// 变更后
let coordinator = kernel::build_coordinator(config, false, None).await?;
```

> TUI 模式不启动 channels，所以直接传 `None`。

---

## 8. 关键设计决策与理由

### 8.1 `SqliteChannelStore` 保留在 kernel

**决策**：`SqliteChannelStore` 不迁移到 `extensions`，而是作为 `kernel::storage` 的一部分保留。

**理由**：
1. 它是纯 SQLite 存储实现，不含任何平台逻辑。
2. `StorageSet` 统一初始化所有存储后端，如果把它移走，需要引入工厂模式或依赖注入，增加复杂度。
3. `extensions` 只需要 `ChannelStore` trait 对象，不需要关心是 SQLite 还是其他实现。

### 8.2 `PlatformConfig` 暂时保留枚举形式

**决策**：`PlatformConfig` 继续保留 `Telegram` 和 `Feishu` 变体，不立即改为动态注册。

**理由**：
1. 完全动态注册（如 `serde_json::Value` 或 `Box<dyn Any>`）需要额外设计配置验证和反序列化机制。
2. 作为第一次拆分，先完成 crate 边界分离，后续引入新平台时再重构 `PlatformConfig` 为动态注册。

### 8.3 `MessageRouter` 的五个方法

**决策**：只暴露 `create_session`、`restore_session`、`get_session`、`send_message`、`subscribe_session_events` 五个方法。

**理由**：
1. 这是 `ChannelHub` 当前使用的全部 `Kernel` 方法。
2. 如果未来扩展需要更多方法（如 `fork_session`），可以逐步添加，不会破坏已有接口。

---

## 9. 实现步骤

### Phase 1: kernel 准备 (无行为变更)

1. **创建 `kernel/src/extension.rs`**
   - 定义 `MessageRouter` trait
   - 定义 `Extension` trait

2. **精简 `kernel/src/channels/mod.rs`**
   - 保留类型、trait、辅助函数
   - 删除 `hub.rs`、`telegram.rs`、`feishu.rs`、`store.rs`、`utils.rs` 的 `pub` 暴露

3. **移动 `SqliteChannelStore`**
   - 从 `kernel/src/channels/store.rs` 移入 `kernel/src/storage/channel_store.rs`
   - 更新 `StorageSet` 的引用

4. **移动 `resolve_safe_path`**
   - 从 `kernel/src/channels/utils.rs` 移入 `kernel/src/utils/path.rs`
   - 删除 `utils.rs`

5. **修改 `Kernel`**
   - 替换 `channel_manager` 为 `extension`
   - 实现 `MessageRouter` trait
   - 更新 `list_channels()`、`shutdown()` 相关方法

6. **修改 `AgentShared`**
   - 替换 `channel_hub` 为 `extension`
   - 更新 `with_channel_manager` 为 `with_extension`

7. **更新 `build_coordinator` / `init_coordinator` 签名**

8. **编译验证 kernel 单独通过**

### Phase 2: 创建 extensions crate

1. **创建 `crates/extensions/` 目录和 `Cargo.toml`**
2. **实现 `extensions/src/channels/mod.rs`**
   - 定义内部模块组织
   - 导入 `kernel::channels` 类型和 trait
3. **移动 `ChannelHub` 到 `extensions/src/channels/hub.rs`**
   - 将内部对 `Kernel` 的引用改为 `Arc<dyn MessageRouter>`
   - 将内部 `build_adapter` 逻辑改为 `ChannelsExtension` 的构造函数
4. **移动 `TelegramAdapter` 和 `FeishuAdapter`**
5. **实现 `extensions/src/lib.rs`**
   - 暴露 `ChannelsExtension`
6. **编译验证 extensions**

### Phase 3: 上层应用适配

1. 更新 `Cargo.toml` 的 workspace 成员
2. 修改 `gui/src/daemon.rs`
3. 修改 `cli/src/commands/daemon.rs`
4. 修改 `cli/src/commands/tui.rs`
5. 全量编译验证

### Phase 4: 测试

1. 运行 `cargo test` 在 kernel 中（确保存储测试通过）
2. 运行 `cargo test` 在 extensions 中（如果有的话）
3. 运行 `cargo clippy --all-targets`
4. 运行 `cargo fmt -- --check`

---

## 10. 风险与回退方案

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| `MessageRouter` 方法不足 | 扩展无法调用某些 Kernel 方法 | 按需扩展 trait，不会破坏已有实现 |
| 上层应用编译失败 | gui/cli 入口需要修改 | 修改量极小（3 个文件，各 3-5 行），已精确列出 |
| 测试覆盖率下降 | 部分 channels 相关测试被移动 | 将测试随实现一起迁移到 extensions crate |
| 第三方使用者（如果有）依赖 `ChannelHub` 类型 | 如果外部 crate 直接引用 `ChannelHub` 类型 | 当前项目无外部使用者，内部修改即可 |

---

## 11. 后续工作

1. **动态平台注册**：将 `PlatformConfig` 从枚举改为动态注册机制，允许新平台通过配置热插拔，无需改代码。
2. **恢复 `SendMessageTool`**：在 `factory.rs` 中取消注释，并正确注册为 `Tool`。
3. **扩展测试覆盖率**：为 `extensions` crate 编写集成测试，覆盖 Telegram 和 Feishu 的模拟交互。
4. **文档更新**：更新 `README.md` 中关于 channels 的架构说明。

---

## 附录 A: 文件迁移清单

| 源文件 | 目标文件 | 操作 |
|--------|----------|------|
| `kernel/src/channels/mod.rs` | `kernel/src/channels/mod.rs` | 保留类型和 trait，删除实现代码 |
| `kernel/src/channels/hub.rs` | `extensions/src/channels/hub.rs` | 移动 + 适配 `MessageRouter` |
| `kernel/src/channels/telegram.rs` | `extensions/src/channels/telegram.rs` | 直接移动 |
| `kernel/src/channels/feishu.rs` | `extensions/src/channels/feishu.rs` | 直接移动 |
| `kernel/src/channels/store.rs` | `kernel/src/storage/channel_store.rs` | 移动 + 更新引用 |
| `kernel/src/channels/utils.rs` | `kernel/src/utils/path.rs` | 合并 `resolve_safe_path` |
| 新建 | `kernel/src/extension.rs` | 新建 `Extension` / `MessageRouter` trait |
| 新建 | `extensions/src/lib.rs` | 暴露 `ChannelsExtension` |
| 新建 | `extensions/Cargo.toml` | 新 crate 配置 |

---

## 附录 B: 接口变更摘要

```diff
  // kernel::build_coordinator
- pub async fn build_coordinator(config, enable_cron, start_channels) -> Result<Arc<Kernel>>
+ pub async fn build_coordinator(config, enable_cron, extension: Option<Arc<dyn Extension>>) -> Result<Arc<Kernel>>

  // kernel::init_coordinator
- pub async fn init_coordinator(config_path, enable_cron, start_channels) -> Result<...>
+ pub async fn init_coordinator(config_path, enable_cron, extension) -> Result<...>

  // Kernel::channel_manager()
- pub fn channel_manager(&self) -> Option<Arc<ChannelHub>>
+ pub fn extension(&self) -> Option<Arc<dyn Extension>>

  // AgentShared::with_channel_manager
- pub fn with_channel_manager(self, manager) -> Self
+ pub fn with_extension(self, extension) -> Self

  // 新增 trait
+ pub trait MessageRouter { ... }
+ pub trait Extension { ... }
```
