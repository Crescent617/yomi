# Channel Watch（/watch 群观察模式）

## 背景

群里的 bot 此前只有被 @ 才醒来：非 mention 消息在 gate 即被丢弃（仅记 passive
receipt），bot 对群里的讨论完全无感。想要的是 bot 作为群的**一个完整在场者**：
读完所有消息，自己判断什么时候该说话——而不是「被 @ 才机械应答」。

## 决策

**一行制：一个 chat 一个 session，mapping 的 `kind` 就是模式开关。**

- `/watch on` 把该 chat mapping 的 `kind` 翻成 `watch`（没有 session 就建）：
  此后每条过 access control 的普通消息（**含 @**）原样镜像（steer）进这个
  session——它是群里唯一的消息消费者，自己决定何时回复（经自己 skill 列表里
  覆盖该平台的 skill，按消息头 `[msg_id: …]` / `[thread: …]` 锚定），没有
  匹配 skill 即纯只读。
- `/watch off` 把 `kind` 翻回 `normal`：**同一个 session** 恢复应答 mention，
  watch 期间的记忆原样保留。没有第三态，没有独立观察者 session，没有
  `watch:` key 前缀——状态就是 `normal` / `watch` 两态。
- 两个方向的 flip 都 cancel 进行中 run + 排干 mailbox：上一模式的待办
  I/O 不得漏进新模式（队列里的对话请求不得在 watch 期间被隐形应答；
  已镜像的消息不得在 off 之后唤醒它）。
- `kind='watch'` 期间 channel 不为它说话：无状态卡、无 streaming、无投递、
  无 reaction、无订阅通知（事件转发器 → delivery pool 一处挡下）。其最终
  文本只落 session transcript（可审计）。说话的唯一形式是 skill 发出的
  普通消息。
- 契约不进首条消息（会被 compaction 摘要掉）：spawn 时 conductor 按 routing
  kind 把 `prompt::watch_section` 追加进 system prompt，免疫压缩。off 期间
  （kind=normal）不追加——它就是个普通会话，拿普通契约。契约正文刻意
  极简，只有三句：处于 watch mode（每条消息镜像给你）、你的正常输出
  无人可见（channel 不代发）、觉得需要回复就经 skill 发言——何时发言
  是 agent 自己的判断，skill 列表与消息头锚点本就在上下文里，契约
  不复述。
- kind 只在建行时写入；reuse 路径只刷新 reply anchor。flip 一律走显式的
  `update_mapping`——并发的对话 dispatch（gate 快照 off、flip 前到达）
  永远不会把 watch 行静默顶回 normal。

被否掉/废弃的备选：

- **独立观察者 session + `watch:{chat}` 前缀 key**（0.10.x 初版）：观察者
  与对话会话并存两行。off 后答 mention 的是 watch 前的旧会话、对 watch
  期间一无所知；前缀本质是把复合键编码进 key 字符串。被一行制取代
  （hrli 拍板）：同一 session 贯穿 普通→watch→普通，上下文连续性完美，
  前缀、第三态、paused 契约变体全部删除。
- **`watch_off` 第三态（paused）**（随初版引入）：想同时保住「记忆复任」
  与「休眠者沉默」就必须存在第三态；一行制下复任白捡（同一 session），
  沉默语义用户不要——off 即普通会话，被 steer/cron 唤醒就以普通会话
  身份说话。
- **独立 `channel_watch` 开关表**（更早的初版）：开关与 mapping 两个生命
  周期两张表，被「kind 即开关」取代。
- **mention 仍走对话会话、观察者只看不答**：双 session 分治丢失单一在场者
  的连贯性，且观察者恰好对最重要的消息（@ 它的）不能答。
- **消抖 buffer**：steer 直进，突发由运行中会话的 mailbox 自然吸收。
- **kernel 侧分类打标 / `watch_skill` 配置校验 / 配置文件开关**：判断材料
  原始消息里全有；skill 列表 agent 自知；per-chat 运行时状态足够。

## 行为

- `/watch`（群聊顶层；DM 与话题内拒绝）查状态；`/watch on|off`（admin）
  切换。watch-on 期间已知命令免 @ 执行（gate 放行，命令回复是 hub 自己的
  声音）；off 后恢复 mention 门控（再开要 @）。
- watch-on 群里：普通消息与 @ 一律无 ack、无状态卡，全部静默 steer 进
  chat 会话；话题消息同样进它（不按 thread key 路由），它按消息头里的
  `[thread: …]` 锚定自己决定要不要经 skill 回进话题。
- `/sessions` 中 kind=watch 的会话标 👁（off 即消失）；`/bind` 在本群
  bind 它被 "Already bound" 短路，跨群重绑 kind=watch 会话被拒绝；
  `/info`（chat 级）watch-on 时附一行状态（观察者 session id）。
- 私聊同样遵守 watch（仅 RPC 可开关；`/watch` 命令仍只在群聊提供）：
  watch-on 的私聊静默镜像、不自动回复，不会形成消息黑洞。
- `/bind` 重绑 watched chat 的会话时 kind 保留（`save_mapping` 只在
  建行时写 kind）：watch 跟行不跟 session，新绑定的 session 接任
  观察者。
- 镜像内容：消息原始 content（adapter 头含 ts/from/chat/**msg_id**/thread/root）
  + 图片 image_key 文本引用（不下载，要用经 skill 自取）。
- daemon 重启：mapping 行在 sqlite，状态自恢复。
- bot 自己（或其它 bot）经 skill 发出的消息会作为事件回推——实测能识别
  自回显并保持沉默（防 loop 靠 agent 判断 + blocked_users）。

## 注意点

- **token 量级**：空闲时每条消息一个「看一眼决定说/不说」的 run（prompt
  cache 压住单价）。中低流量群适用；高流量群熔断（每小时 run 上限）留作
  后续。
- **注入面**：触发面 = 群里每句话；watch 会话以 Dangerous 自动批准 + 平台
  skill 在手（能以 bot 身份发言）。blocked_users 与 access control 是硬
  防线；「channel 不代发」是硬边界，「只用 skill」是 prompt 级约束。
- **gc 边界**：watch 会话被 gc（默认 90 天静默）后 mapping 行随之删除，
  watch 静默关闭，需要重新 `/watch on`。
- **投递路由缓存 ≤2s**：事件转发器按 session 缓存 routing（含
  kind）。flip 后 2s 内新 run 的输出可能沿旧 kind 被抑制（或放行）
  ——flip 的 cancel+drain 已清在途，影响仅限 2s 内的新 run，记为
  已知边界。
- **`/watch on` 时若有进行中的对话 run**：flip 即 cancel（对称 off），已
  发出的状态卡可能停在半更新态——cosmetic 边界，flip 是低频管理操作。
- **观察者看不到自己经 skill 说的话之外 bot 的消息**：bot 消息不回推本
  bot——off 期间由它自己应答 mention，此洞不存在。

## 实现

- `channels/mod.rs`：`MappingKind` 两态（`'normal'|'watch'`，迁移 v24 加
  列；遗留 `watch_off` 由 `from_str_lossy` 归入 normal，无数据迁移）；
  store trait：`find_mapping_kind`（一次读回 session+kind）、
  `update_mapping`（anchor 刷新 / kind flip 的显式通道）、
  `list_watch_sessions`（👁）；`save_mapping` 的 upsert 不写 kind
  （insert-only，不变量由构造保证）。
- `channels/hub/watch.rs`：`mirror_message`（tee 本体：单次 route 锁内
  重读实时行 + steer——与 flip 的 read-flip-reset 同锁互斥，off/gc
  不得插队；行 kind=watch 即走 `get_or_create_session_locked`（存活
  =reuse，悬空=删+建，同临界区），行不在=丢弃不重建）；
  `{get,set}_channel_watch_by_name`（查询/开关核心，slash 命令与 RPC
  共用；无状态变化=纯 no-op，off 绝不误杀普通会话）；hub 薄封装 +
  `rpc_set_channel_watch`（`on` 缺省 = 查询，Vim `:set` 风格）。wire
  `SetChannelWatch`（proto 维持 28）。
- `channels/hub/gate.rs`：watch-on 群普通消息与 @ 全 `NotAddressed`（静默），
  已知命令放行；watch 快照随消息进 dispatch（单一读取，无 toggle 竞态）。
- `channels/hub/mod.rs`：dispatch 循环 tee（gate 快照 && `Command::None`
  → `mirror_message`）；事件转发器 `routing.is_watch() → continue`（投递
  抑制单点）。
- `channels/hub/routing.rs`：`get_or_create_session`（取锁外壳）+
  `get_or_create_session_locked`（内核，契约：调用方须持 route 锁——
  g_lock 不可重入，供 tee/flip 的宽临界区复用）；复用前校验 session
  存活，悬空行删除重建；kind 只在建行时写入，reuse 仅刷新 anchor。
- `kernel/mod.rs`：`delete_session` 级联删除 channel mappings。
- `kernel/conductor.rs` + `prompt/mod.rs`：spawn 按 routing kind==Watch
  追加 `watch_section` 契约。
- `channels/hub/handlers.rs`：`/watch`（admin、chat 级）；`/bind` 拒绝跨群
  重绑 watch 会话、`/sessions` 👁、`/info` watch 行。
- `platform/feishu_events.rs` / `telegram.rs`：消息头增 `[msg_id: …]`。
