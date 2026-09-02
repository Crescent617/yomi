# Session 级 Context Window 配置（GUI / TUI / channel settings card）

## 目标

每个 session 可覆盖模型的 `context_window`：GUI、TUI、channel
`/settings` 卡三个面都能查/改/复位，改动即刻影响该 session 的压缩触发点、
provider 输入自检与 ctx% 展示。

非目标：不动 channel/agent 级默认（配置文件的 `[[models]].context_window`
仍是默认来源）；不动 `threshold_ratio`、`max_tokens`；不做 per-channel
默认值层。

## 现状（为什么改动可以很小）

`context_window` 的全部消费点都从一个**唯一解析出口**拿值：

- `AgentShared::resolve_model(session_id)`（agent/types.rs）：读
  `sessions.model_key`（NULL→默认模型）→ 注册表取 `ModelConfig` →
  provider + config 返回。每 turn 开头重新解析。
- 下游消费：compactor 触发阈值（`compactor::should_compact`）、provider
  输入预算自检（`provider/mod.rs` 的 insufficient-context 报错）、
  `TokenUsage` 事件携带的窗口值（obs 卡 / 回复尾 ctx% / TUI 状态栏）。

模型覆盖的持久化范式已存在：`sessions.model_key`（迁移 v13，NULL=跟随
默认）、`update_model_key`/`clear_model_key`、thread session 经
`model_key_for_new_channel_session` 继承 chat session 的显式选择、fork
复制元数据、`/model`（chat 级扇出）与 settings 卡 `cfg_model` 行、GUI
`ModelSelector`（`get/set_session_model`）、TUI `/models` picker。

**设计 = 把 `model_key` 的一整套模式按 `context_window` 复制一份。**

## 决策

### 存储与生效

- `sessions` 加一个**通用 settings 袋**：`settings TEXT NULL`（迁移
  v25）——JSON object，`NULL` = 无任何覆盖。此后 session 级旋钮都进
  这个袋，**不再一个旋钮一列**（现状 working_dir → project_id →
  auto_approve_level → model_key → template → tools_block 已是六次
  滴漏迁移；既有列不搬家，无 churn）。
- 应用层 typed 视图：`SessionOverrides { context_window: Option<u32>,
  … }`（serde，只读已知 key；更新走 SQLite `json_set`/`json_remove`
  **原子按键写**——GUI 与 settings 卡并发改不同 key 互不踩，且不经
  读-改-写序列化，未来新 key 不会被旧 daemon 吃掉；袋空归一为 NULL：
  `NULLIF(json_remove(…), '{}')`）。JSON 袋是纯存储细节：wire/RPC
  保持 typed 方法，kernel 校验，拒绝幽灵 key。
- 生效点唯一：`resolve_model` 取出 `ModelConfig` 后，若
  `settings.context_window` 有值则 clone 替换再返回。下游零改动。
  （每 turn 重解析，改动即刻生效，进行中的 run 下一 turn 适用。）
- 优先级：`session override > 模型配置`。不做第三层。
- 校验：`> 0` 即收。调低恒安全（提前压缩）；调高把"真实上限以
  provider API 为准"的裁量交给用户（配置可能落后于模型升级）——超界
  会被 API 400 拒掉，报错可见。provider 自检用的是同一个被覆盖值，
  调大会同步放宽自检，文档与设置面都要把这句写明白。

### 换模型时的交互

override **不**随 `/model` 切换清除（explicit-is-explicit，与
`model_key` 同哲学）。所有设置面显示 `effective vs 模型默认`，覆盖值
大于新模型窗口时风险可见。被否掉的备选：切换时清除（破坏"显式即显式"
一致性）；`min(override, 模型默认)` 钳制（直接杀死合法的调高场景）。

### Thread 继承

与 model_key 完全对称，两个方向各自生效：

- **现存会话**：chat 级改动（settings 卡）扇出写 chat session 与该
  chat **现存**全部 thread session（个别 session 已删/陈旧 mapping
  不中断扇出，warn 跳过）。
- **未来会话**：新 thread session 建行时经
  `overrides_for_new_channel_session` 继承 chat session 当时的显式值
  （model_key 与 settings 袋的 context_window）。

fork 整袋复制（settings 是一列，天然跟随 fork 的元数据复制）。

### 各端改法

**wire / RPC（proto 不升版，纯新增——先例 get_rules）**

- `GetSessionContextWindow { session_id }` →
  `{ effective, override: u32|null, model_default, model_key }`。
- `SetSessionContextWindow { session_id, tokens: u32|null }`——
  `null` 清除覆盖（复位为跟随模型）。
- kernel 侧同名 API + SessionStore 的通用袋读写
  `set_session_setting(id, key, value)` / `remove_session_setting(id, key)`
  （内部 `json_set`/`json_remove`，kernel API 只暴露 typed 旋钮）。

**Channel settings 卡（chat scope）**

第 4 行 "Context window"，`select_static` 离散档位（卡片没有自由输入）：
当前会话模型窗口的 **25% / 50% / 75% / 100%** + 伪选项
`default (Nk)`（清除覆盖）。回调 `cfg_ctx` 走 `set_chat_context_window`
（新，镜像 `set_chat_model` 扇出：写 chat session + 该 chat 现存全部
thread session）。♻️ Reset all 一并清除。行 label 即 `Context window`
（生效值由选中项表达——档位/`custom (Nk)`/`default (Nk)` 与之恒等价，
不再挂 `now` 后缀）。

任意精确值走 GUI/TUI/CLI（卡片只做档位，不为卡片造输入框）。

**GUI**

编辑入口放 **input 工具条的 ctx 仪表**（与 PermissionSelector /
ModelSelector 同排——感知点即操作点，现有 per-session 旋钮全在这排）：
仪表从被动指示改为 button，点击弹 popover（复用 ModelSelector 的
dropdown/click-outside 范式）。popover 内容：生效值 + 来源行
（`override 400k · 模型默认 800k`）→ 25/50/75/100% 快捷档 → 自定义
输入（`512k`/`1M`/纯数字，`parse_number_with_unit`）→ Reset（清除
覆盖）。仪表的 tooltip 同步带生效值/来源；ctx% 经 TokenUsage 事件原地
反馈。

会话信息面板（Rules 区块旁）加一行**只读**展示：生效值 + 来源
（override 还是跟随模型默认）——编辑态仍只在工具条 popover，面板不
做第二个编辑入口。

新 tauri commands：`get/set_session_context_window`（薄封装 client）。

**TUI**

新命令 `/ctx [value]`：无参查当前（effective + 来源）；`/ctx 512k`
设置；`/ctx reset` 清除。状态栏 ctx% 经 TokenUsage 自动生效。
（模型 picker 不动——ctx 与模型是两个正交旋钮。）

**CLI（parity，顺手）**

`yomi session ctx <session_id> [value|reset]`，无参查询。

### 文档

- 本设计文档；CONFIG.md 提一句 session 级覆盖入口。
- 顺带修 `compactor/README.md` 的过期表述：现行代码 `threshold()` 是
  `min(ratio×window, window−remaining_reserve, window−summary_reserve)`，
  没有 110k 硬顶（README 的 "110_000 tokens" 一行是旧语义）。

## 边界与风险

- **调高超界**：API 400 可见报错；不提前拦截（理由见校验决策）。
- **observer/watch session**：同样是 session，同规则覆盖。
- ** compaction 基线**：改动窗口不清历史、不重置 token 基线——压缩估计
  每 turn 重算，方向安全（调低最多多压一次）。
- **进行中 run**：下一 turn 生效（resolve_model 每 turn 重解析），不打断
  流式。
- **`updated_at` 副作用**：写 settings 袋触碰 `updated_at`（与
  `update_model_key` 先例一致）——改 ctx 会把 session 顶到列表前、推迟
  gc 过期。接受（一致性优先），后续若要治理应两个旋钮一起改。

## 测试计划

- kernel 单测：解析优先级（override > 模型默认；清除后回落）、持久化
  往返、fork 复制、thread 继承（chat 显式值 → 新 thread session 建行
  带出）、换模型后 override 保留。
- compactor：覆盖值传入 `should_compact` 的阈值变化（调低提前触发）。
- settings 卡：第 4 行渲染（档位/伪选项/当前值）、`cfg_ctx` 回调映射
  （档位设置、default 清除、未知 option 不动）、Reset all 联动。
- wire：两方法 dispatcher roundtrip。
- 真链路（yomi-e2e）：settings 卡切档位 → 该 chat session 回复尾 ctx%
  分母变化；TUI/GUI 手动验证。

## 实现顺序建议

1. storage 迁移 + `settings` 袋读写（`json_set`/`json_remove` 原子按键
   更新）+ SessionInfo/SessionOverrides + fork（纯数据层）
2. resolve_model 生效点 + kernel API + wire/RPC + dispatcher
3. channel：`set_chat_context_window` + settings 卡行/回调 + Reset all
4. TUI `/ctx`；5. GUI 面板区块；6. CLI 子命令；7. 文档 + CHANGELOG
