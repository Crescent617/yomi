# 设计文档：Extension 一期 —— custom tool 与 source

源自 2026-08 的 dsh/Cordis 调研与 extension 四端口讨论（见 AGENTS.md《Design
Philosophy》）。一期只做两件事：**外部进程经 wire 协议注册 custom tool**、
**source 经 `route_message` 获得会话路由**。gate（外挂裁决）整体挪到二期，
sink 复用现有 `subscribe_*`，channel 插件化等第三个渠道出现再说。

## 第一性原理

**一个扩展 = 一条 wire 连接 + 一本副作用账本。**

- 注册即记账：每个连接一张 ledger（registration 集合），不做持久化
- 杀进程即下掉：连接断开 → 账本逆序回收（tool 摘出 ToolRegistry、
  pending 工作项全部报错）
- 状态只存内存：daemon 重启 = 注册表清空，扩展重连重注册——
  注册表本来就是 fold 出来的（状态是缓存）

没有插件管理器、没有生命周期 RPC、没有注册持久层。

## 范围

| 端口 | 一期 | 二期+ |
|---|---|---|
| Capability | ✅ `ext_register`/`ext_pull`/`ext_result`（custom tool） | — |
| Source | ✅ `route_message`（pseudo-channel 映射复用） | webhook 收编为 channel adapter |
| Gate | ❌ | 外挂裁决（`pre_tool_use` 等，pull 同形） |
| Sink | 复用现有 `subscribe_*`（零改动） | 分类型过滤器 |
| Channel | ❌ | 第三个渠道出现时 |

## 协议（wire 新增 5 个方法 + send_message 扩展，版本 → 28）

### `ext_register`

```json
→ {"ext_register": {"kind": "tool", "name": "stock.quote",
    "desc": "查询股票实时价格",
    "schema": {"type":"object","properties":{"symbol":{"type":"string"}},"required":["symbol"]},
    "level": "safe"}}
← {"ok": {"registration": "ext_01J..."}}
```

- `kind: "tool"` 必填；`level` 缺省 `caution`（走审批）。
- name 全局唯一（含内建工具）；撞名报错。tool_blocklist 对 ext 工具同样生效。
- 注册进 ToolRegistry 的是一个**代理 Tool**：desc/schema 用登记的，
  exec 时把调用派给登记连接的队列。

### `ext_unregister`（可选，优雅下线单个能力）

```json
→ {"ext_unregister": {"registration": "ext_01J..."}}
← {"ok": null}
```

### `ext_pull`

```json
→ {"ext_pull": {"registration": "ext_01J...", "timeout_ms": 55000}}
← {"ok": {"call_id": "c_8f2", "name": "stock.quote", "args": {"symbol": "600519"}}}
   {"ok": null}   // 超时（默认 55s，上限 60s）
```

- per-registration 一个 `VecDeque` + `Notify`；有单即取、无单挂起（**挂起等，不空转**）。
- 一个工作项只 pop 一次（恰好一次是结构保证）；同一 registration 允许并发 pull（多 worker）。
- 工作项状态机：`queued → delivered → resolved / expired（60s 无响应）
  / cancelled（run 被 Stop）`；迟到的 result 丢弃并记事件。

### `ext_result`

```json
→ {"ext_result": {"call_id": "c_8f2", "output": "1900.00", "is_error": false}}
← {"ok": null}
```

### `route_message`

```json
→ {"route_message": {"source": "gitlab-ci", "key": "proj123/pipelines",
    "target_hint": {"channel": "feishu", "chat_id": "oc_devops"}}}
← {"ok": {"session_id": "sess_...", "created": false}}
```

- 复用 channel mapping store：source 名当 pseudo-channel（第一维）、
  key 当 mapping key（第二维）。会话创建/复用/gc 级联全免费。
- 三种模式：固定 session（config 写死）、话题 keyed（source+key 自动建/复用）、
  挂靠渠道会话（`target_hint` → 回复走该会话的渠道出向，如回飞书群）。
- 之后照常 `send_message`；source 消息统一带 `[From source:<name>]` 前缀，
  对齐 channel 消息的来源标注惯例。

### Source 回执：`send_message` 的 `wait` 选项

source 经常需要"发完消息拿结果"（如把 agent 答复回贴到 GitLab MR
comment）。不新增方法，给 `send_message` 加两个可选参数：

```json
→ {"send_message": {"session_id": "sess_x", "blocks": [...],
    "wait_ms": 300000, "client_tag": "mr-8842"}}
← {"ok": {"text": "……", "stop_reason": "completed",
           "timed_out": false, "client_tag": "mr-8842"}}
```

- RPC 挂起直到该消息触发的 run 结束（挂起等，与 ext_pull 同构），返回
  **run 的最终答复**——与渠道投递（settle 正文）同一份计算，不二门。
- session 忙时消息排 mailbox，wait 覆盖排队+执行；超时 `timed_out: true`，
  run 继续不取消（v1 语义）。
- `client_tag` 原样回显，供 bridge 对齐外部对象（如 MR 号）。
- mailbox 合并语义：消息与他人合并进同一 run 时，返回该 run 的最终答复
  （v1 接受并记录）。
- 高级需求（流式进度、多轮跟踪）走现有 `subscribe_session_events`。

## 生命周期与回收（三层，零管理面 RPC）

1. 显式：`ext_unregister`
2. 连接断开：该连接的账本逆序回收——代理 tool 摘除、pending 工作项
   以 "tool provider disconnected" 报错给调用方。**杀进程即下掉，主路径。**
3. daemon 重启：内存表清空；supervised 自动重拉，external 各自重连重注册。

## Config（supervised 模式，可选）

```toml
[[extensions]]
name = "stock-tools"
command = ["uv", "run", "~/.yomi/ext/stock_tools.py"]
autostart = true          # 默认 false
restart = "on-failure"    # 默认 no
```

supervised 由 daemon spawn，进程组管理（复用 background shell 组杀语义，
daemon 死则全组 SIGTERM）。supervised 与 external 注册契约一致——
启动方式只是运维差异（内外同构），config 可以为空。

## 收口与防幽灵

- 代理 tool 经 ToolRegistry：permission 分级、tool_blocklist、审批卡、
  ToolEvent 全部自动生效——模型分不出内建外供。
- `ext.register/unregister/dispatch/expire` 发边界事件（Sink 可观测）；
  pull/result 不发事件（太吵）。
- unix socket 即认证（文件权限）；tcp 化留待远程需求，届时加 token。

## 改动文件（预计）

- `crates/kernel/src/wire/mod.rs`：5 个 ReqMethod + 版本 28
- `crates/kernel/src/extension/`（新）：registry、代理 Tool、pull 队列、
  连接 sweep 钩子
- `crates/kernel/src/server/{dispatcher,connection}.rs`：方法分发 + conn drop sweep
- `crates/kernel/src/kernel/conductor.rs`：route_message 的 pseudo-channel 映射
- `crates/kernel/src/config/mod.rs`：`[[extensions]]` 段
- `crates/cli`：`yomi ext list`（查看登记，debug 用，可缓）
- `ext/sdk/yomi_ext.py`（repo 外或 examples/）：~50 行 Python SDK

## Python SDK 形状

```python
from yomi_ext import Ext

ext = Ext()  # unix socket 连接 + Hello
ext.tool("stock.quote", "查询股票实时价格",
         schema={"type":"object","properties":{"symbol":{"type":"string"}},"required":["symbol"]},
         level="safe")

@ext.on("stock.quote")
def quote(args):
    return {"price": fetch(args["symbol"])}

ext.serve_forever()  # pull → dispatch → result 循环；断开即退出
```

## 狗食计划

1. `~/.yomi/ext/stock_tools.py` 注册 `stock.quote`（stock-pool 数据）
2. 飞书里让 agent 调用，验证：schema 进模型工具表 → 审批（caution）→
   pull 派单 → Python 执行 → ToolEvent 完整 → 结果回模型
3. 杀掉 Python 进程，验证工具立即从工具表消失、后续调用报 disconnected
4. route_message：webhook 桥（GitLab CI payload → transform → 挂靠飞书群）

## 已知弃选项（记录在案）

- **双向 RPC（server→client 主动调用）**：v1 用 pull 倒转 request/response，
  保持 wire 永远 client 主动。触发重估的条件：需要 daemon 主动取消在飞的
  外部调用（Stop 传播），或 provider 数量多到长连接成负担。
- **注册持久化**：明确不做。重连重注册是契约的一部分。
- **gate 一期**：拦截点要动 conductor 热路径，单独一批做（二期）。

## 状态

待实施（2026-08-21 设计定稿）。
