# 设计文档：Extension 一期 —— custom tool 与 source

源自 2026-08 的 dsh/Cordis 调研与 extension 四端口讨论（见 AGENTS.md《Design
Philosophy》）。一期只做两件事：**外部进程经 wire 协议注册 custom tool**、
**source 经 `route_message` 获得会话路由**。gate（外挂裁决）整体挪到二期，
sink 复用现有 `subscribe_*`，channel 插件化等第三个渠道出现再说。

## 第一性原理

**一个扩展 = 一条 wire 连接 + 一本副作用账本。**

- 注册即记账：每个连接一张 ledger（registration 集合），不做持久化
- 杀进程即下掉：连接断开 → 账本逆序回收（tool 摘出 ToolRegistry、
  pending 工作项全部报错）——**teardown 只有断开连接一条路（RAII），
  不设 unregister RPC**；改 schema/改名 = 重连重注册，是契约的一部分
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

## 协议（wire 新增 4 个方法，版本 → 28）

方法面：`ext_register` / `ext_pull` / `ext_result` / `ext_route`。
不设 `ext_unregister`（断开即回收，RAII 覆盖一切下线场景）。

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

### `ext_pull`

```json
→ {"ext_pull": {"registration": "ext_01J..."}}
← {"ok": {"call_id": "c_91a", "name": "stock.quote", "args": {"symbol": "600519"}}}
   {"ok": null}   // 超时（默认 55s，上限 60s）
```

- per-registration 一个 `VecDeque` + `Notify`；有单即取、无单挂起（**挂起等，不空转**）。
- 一个工作项只 pop 一次（恰好一次是结构保证）。
- **单 worker 约束**：同一 registration 同时只允许一条挂起 pull，第二条
  直接报错；要并发就多开进程（多连接多 registration），扩容路径天然且隔离。
- 超时固定 55s（空转心跳，不设参数）。
- 工作项状态机：`queued → delivered → resolved / expired（60s 无响应）
  / cancelled（run 被 Stop）`。

### `ext_result`

```json
→ {"ext_result": {"call_id": "c_91a", "output": "1900.00", "is_error": false}}
← {"ok": null}
```

- call_id 必须属于本连接本 registration（防串线），否则报错（client bug）。
- 迟到的 result（expired/cancelled 后到达）丢弃并记事件——**独立失败域**：
  result 的任何问题都不影响 pull 循环。

### `ext_route`

```json
→ {"ext_route": {"source": "gitlab-ci", "key": "proj123/pipelines"}}
← {"ok": {"session_id": "sess_...", "created": false}}
```

- 复用 channel mapping store：source 名当 pseudo-channel（第一维）、
  key 当 mapping key（第二维）。会话创建/复用/gc 级联全免费。
- 之后照常 `send_message`；source 消息统一带 `[From source:<name>]` 前缀，
  对齐 channel 消息的来源标注惯例。
- **回复出向**（如回飞书群）两条路，均无需协议新增：
  ① bridge config 直接写目标群的 session_id（人工查一次 mapping 即可，
  然后**根本不调 ext_route**，直接 `send_message`）；
  ② source 会话 prompt 约定"结果发到 oc_xxx"，agent 用 lark skill 自行转发。
- ext_route 只为**话题 keyed** 路由存在；固定 session 不是它的模式，
  是不用它的场景。

### Source 回执（一期不做）

source"发完消息拿结果"（如回贴 GitLab MR comment）的需求一期不处理：
`send_message` 的 wait 语义碰上 mailbox 排队/合并/Stop 全是毛边，而
webhook 场景天然异步（回贴晚到几分钟无影响）。需要结果时用现有
`subscribe_session_events` 订阅事件流自行跟踪 run 生命周期；
待真实场景出现再评估是否值得同步原语。

## 生命周期与回收（两条路，零管理面 RPC）

1. **连接断开（唯一主动路径，RAII）**：该连接的账本逆序回收——代理
   tool 摘除、pending 工作项以 "tool provider disconnected" 报错给调用方。
   杀进程即下掉。
2. **daemon 重启**：内存表清空；supervised 自动重拉，external 各自
   重连重注册。

## 收口与防幽灵

- 代理 tool 经 ToolRegistry：permission 分级、tool_blocklist、审批卡、
  ToolEvent 全部自动生效——模型分不出内建外供。
- `ext.register/unregister/dispatch/expire` 发边界事件（Sink 可观测）；
  pull/result 不发事件（太吵）。
- unix socket 即认证（文件权限）；tcp 化留待远程需求，届时加 token。

## 改动文件（预计）

- `crates/kernel/src/wire/mod.rs`：4 个 ReqMethod + 版本 28
- `crates/kernel/src/extension/`（新）：registry、代理 Tool、pull 队列、
  连接 sweep 钩子
- `crates/kernel/src/server/{dispatcher,connection}.rs`：方法分发 + conn drop sweep
- `crates/kernel/src/kernel/conductor.rs`：route_message 的 pseudo-channel 映射
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

ext.serve_forever()  # pull → dispatch → ext_result 循环；断开即退出
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
- **result 搭 pull 便车（piggyback）**：弃选（2026-08-21）。过期 result 是
  常态（Stop/超时），piggyback 下会误杀取活循环——失败域耦合；
  且 daemon 队列本就吸收"交结果与下一拉"的缝隙，原子性无收益。
- **ext_unregister RPC**：弃选。断开即回收（RAII）覆盖一切下线场景。
- **注册持久化**：明确不做。重连重注册是契约的一部分。
- **gate 一期**：拦截点要动 conductor 热路径，单独一批做（二期）。
- **supervised config（`[[extensions]]`，daemon 拉起扩展进程）**：一期不做
  （2026-08-21）。nohup/launchd 足够；加回条件：扩展常驻化需求明确，
  届时复用 background shell 的进程组管理。
- **多 worker（同 registration 并发 pull）**：一期单 worker，第二条挂起
  pull 报错；扩容 = 多进程多连接。
- **`ext_pull` 的 timeout_ms 参数**：固定 55s，不设 knob。
- **`ext_route` 的 target_hint（挂靠已有渠道 chat）**：弃选（2026-08-21）。
  双重归属语义（session 同属群聊与 source key）+ 映射冲突处理是隐藏税；
  固定 session_id 与 agent 经 lark 转发已覆盖出向需求。加回条件：
  bridge 需按事件内容运行时动态选择目标群。

## 状态

待实施（2026-08-21 设计定稿）。
