# 设计文档：cross-chat relay —— 跨群提问的答复回家

> **状态（2026-09-13 hrli 拍板）：暂缓**。带内 hint 的 skill 方案
> （lark skill「跨 chat 提问」节；此前的独立 skill
> `cross-chat-relay` 已删、内容并入 lark skill）在复杂度/可靠性上已
> 够用：零 kernel 改动、零 registry、接收方零查找，且群侧回执天然自
> 洽。本文档存档为备选——当跨群问答变高频、或投递需要系统级硬保证
> 时再启动，启动时先复审「决策记录」各条。
>
> 附带落地：post 消息提取改读 `content_v2` 原始 markdown（含 `[X](X)`
> 裸链接去重，feishu_text.rs `extract_post_text`）——quoted/history
> 注入从此保留链接 URL（含 yomi:// 自定义 scheme），是上述 skill 方案
> 「指令藏 URL」能成立的 kernel 前提。

会话在 A 群 thread 工作，中途需要去 B 群（或私聊）向人提问；B 群的人
@ bot 作答后，**工作会话要收到这份答复**。本文档定义该能力的 kernel
实现。skill 层 interim 方案（`.agents/skills/cross-chat-relay`，带外
SQLite registry + 引用回复 + post_message 转发）已于 2026-09-13 四轮
实测验证语义可行，但其发现环节要求**每个会话对每条引用回复主动查
registry**——用 LLM 概率补系统缺口，是本文档要消除的东西。

## 问题

入站消息按 `(channel, mapping_key)` 路由（routing.rs）：thread 内消息
以话题 root 为 key，群主流消息以 chat_id（或 rit 模式下消息自身）为
key。bot 经 API 直发到 B 群的提问不产生任何 mapping，B 群的答复只会
路由到 **B 群自己的会话**（或新建话题会话），工作会话永远感知不到。

## 语义决策：投递（delivery），不是路由翻转（route flip）

两个候选语义：

- **路由翻转**（bind_mapping）：把提问消息 id 绑到来源会话的 mapping，
  答复直接路由进来源会话。被否：B 群话题的会话所有权被永久转移，来源
  会话的后续回复会渲染到 B 群，一个会话两个锚点，会话一对一模型被糊
  掉；且 quote-reply 路径仍要为显式绑定单开特例。
- **投递**（本文档）：答复作为一条 steer 消息**送回**来源会话，来源会
  话的家与锚点不动；想继续对话经下行通道显式回话。"拿答复"与"继续对
  话"拆开，问答是一次外出跑腿而非搬家。

投递分两种期限：**one-shot**（默认，首条答复送达即 consumed，后续消
息落回正常路由）与 **sticky**（该提问话题的后续消息持续投递，直至
显式 release）。

## 群侧一致性（B 群可见体验）

裸投递有一个缺口（2026-09-13 hrli 指出）：答复被拦截送走后，B 群的
人 @ 了 bot 却得不到任何回应；one-shot consumed 后的追问落进 B 侧会
话，而该会话对这个问答一无所知（提问带外发出、答复被拦截），只能
莫名应对。skill 方案恰好无此病——B 侧会话就是经手人，本地回执、本
地有上下文。补法三件套：

1. **命中即自动回执**：relay 命中时 kernel 立即以 bot 身份在 B 群回
   一条最小 ack（"✅ 已转达"），答题人当场得到闭环反馈。
2. **ack 兼作面包屑**：ack 留在话题历史里自我记录。后续追问落进 B
   侧会话时，rit thread 会话的话题历史回填（context.rs）让它看到
   "bot 问过 → 人答过 → bot 已转达"的完整场面——无需知晓 relay 内
   部即可得体应对。
3. **来源会话的回话路径**：投递消息附答题消息的 msg_id，来源会话要
   实质回话经 `channel_send --reply-to` 回到 B 群话题，群侧叙事完
   整：bot 提问 → 人答 → bot 回执 →（可选）bot 回来认真答复。

sticky 期限内追问持续投递，回执同样每条都给。

## 决策记录（2026-09-13，与 hrli 讨论；标注拍板状态）

1. **kernel 化，skill 退场**（hrli 拍板："这个 skill 还太重了，要求
   每一次发消息都检查一下，这个太难了"）。skill 保留至 P2 上线后删。
2. **投递语义**，route flip 被否（理由见上节；hrli 未异议，待确认）。
3. **不设兜底轮询**（hrli 拍板）：对方另发新消息（不引用不进话题）
   抓不到，是语义边界而非缺陷；发送方主动跟进时 `lark im read` 自查。
4. **消息 UI 零标记**（hrli 拍板）：映射关系全部落 kernel 存储，不带
   在消息文本里。
5. **终态入口 = `channel_send {relay:true}`**：发信收归 kernel 后登记
   成为发信副作用，意图/身份/登记在一次调用里塌缩，agent 零协议负担
   （提议，hrli 问询后未拍板）。
6. **首发暴露面 = CLI**（`yomi channel send --relay` + relay 管理子
   命令），与 `channel new-thread` 先例一致；内置 agent 工具留作观察
   后的可选项（提议）。
7. **one-shot 为默认期限**；sticky 仅在注册时显式声明（提议）。

## 存储

migration 新增（storage/migrations.rs）：

```sql
CREATE TABLE channel_relays (
  channel TEXT NOT NULL,
  question_msg_id TEXT NOT NULL,     -- 提问消息的外部 id（om_…）
  source_session TEXT NOT NULL,      -- 等答复的会话
  chat_id TEXT NOT NULL,
  sticky INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  consumed_at TEXT,                  -- NULL = 在途
  PRIMARY KEY (channel, question_msg_id)
);
```

注册冲突（同 key 重复 register）：保留先注册者，返回已存在标记——
后注册者覆盖会把答复偷去别的会话。

## 入站挂钩点

hub dispatch 串行循环内，**mention 门槛之后、路由解析之前**：

```
过 mention 闸的 msg
 → relay 查找：msg.root_id 命中？msg.parent_id 命中？   # 仅查显式登记行
    ├─ 未命中 → 现有路由（effective_mapping_key），行为零变化
    └─ 命中且来源会话存活
         → 消息以 steer 注入 source_session：
           [relay 答复 · <chat 名> · <发送者>] <原文>（msg link）
         → one-shot：标 consumed_at；sticky：保留
    └─ 命中但来源会话已删 → 删行，落回现有路由（不黑洞，
       同 get_or_create_session 的 dangling-mapping guard 思路）
```

两个查询键覆盖人类全部"针对性回复"方式：话题内回复（`root_id` = 提
问消息）与主流引用回复（`parent_id` = 提问消息）。**只查显式登记
行**，不触碰"plain quote-reply 开新会话"的既有设计（routing.rs
`session_mapping_key` 注释），非 relay 消息的路由行为逐字节不变。

投递进来源会话的消息沿用现有 steer 通道（conductor 的 AgentInput），
由挂钩点直接构造，不走路由触发的 history/quoted 前缀装配——提问原文
本来就在来源会话上下文里，无需注入；不新增会话内消息类型。

## 接口

wire RPC（单点实现，dispatcher `rpc_body` 模式）：

- `relay_register {channel, chat_id, message_id, sticky?} → {created}`：
  供 P1 配合带外发信（lark CLI）使用。
- `relay_release {channel, message_id}` / `relay_list {channel?, chat_id?}`：
  管理与排查。
- `channel_send {channel?, chat_id, text, reply_to?, relay?, sticky?}
  → {message_id, link}`（P2）：经 adapter 发信，发送身份即 channel 自
  有 bot；`relay:true` 时用返回的 message_id 自动写 `channel_relays`
  （sender session 取 RPC 调用方会话）。

CLI（RPC 薄前端，同 `channel new-thread` 先例）：

- `yomi channel send --chat <oc_> --text … [--relay] [--sticky] [--reply-to <om_>]`
- `yomi channel relay list [--chat <oc_>]` / `yomi channel relay release <om_>`

agent 教学走 yomi-self skill 增补一节，不进内置工具面（理由见决策 6）。

## 权限

`channel_send` 是有外部副作用的出站发信，闸做在 RPC 层、与暴露面无
关（决策记录外补充，待 hrli 拍板）：**目标 chat 无任何现存 mapping**
（bot 从未在该群有过会话）时要求确认——CLI 交互确认 / RPC 调用带
`approve:true` 显式声明；有 mapping 的群视为熟群直发。relay 注册本身
无出站副作用，不设闸。

## 平台

- feishu：`root_id`/`parent_id` 事件字段齐全，两键均可查。
- telegram：无 thread，`reply_to_message` 映射到 `parent_id` 单键可查；
  语义不变。
- mention 门槛照旧：答复方在 require_mention 的群仍需 @ bot 才触发。

## 测试计划（hub_test 现有模式可套）

1. 话题内回复命中 relay → steer 注入来源会话，B 群会话无消息，B 群
   出现自动 ack。
2. 主流引用回复（parent_id 键）命中 → 同上。
3. 未登记消息的 thread/quote 回复 → 路由行为与现状完全一致（回归），
   无 ack。
4. one-shot consumed 后第二条答复 → 落回正常路由进 B 群会话，其话题
   历史回填含"提问+答复+ack"（群侧一致性，测试 2 的 ack 在场）。
5. sticky 连续两条答复 → 均投递且均有 ack，release 后落回。
6. 来源会话已删 → 行被删、消息落回正常路由、无 ack。
7. 重复注册同 key → created=false，先注册者不变。
8. `channel_send --relay` 端到端：发信→登记→答复投递（feishu 适配器
   mock，复用 hub_test 的 MockAdapter）。

## 分期

- **P1**：表 + 入站挂钩 + `relay_register/release/list` RPC + CLI 管理
  面。配合 lark CLI 发信即可用（发信带外，登记带内）。
- **P2**：`channel_send` RPC + `yomi channel send --relay`，发信收归
  kernel，登记自动化；权限闸随此项落地。skill 此时删除。
- **P3（可选）**：内置 `channel_send` agent 工具（观察 CLI 发现性后再
  定）；telegram 侧实测。

## 已验证的语义原型（2026-09-13，skill 实测记录）

四轮真机测试（feishu，任务分派中枢群 + 审批群）：带内自描述标记与
带外 registry 两方案均端到端闭环；关键实证——①被引用消息原文经
`<quoted_message>` 注入必然到达接收方上下文（bot API 直发的消息恒为
缺失上下文）；②引用回复消息头恒带 `root_id` = 被引用消息 id（投递
挂钩的查询键可靠）；③零上下文全新会话可被协议驱动完成转发（27 秒）。
③同时暴露了 skill 方案的发现环节靠 LLM 自觉，即本文档 P1 挂钩要替
代的部分。
