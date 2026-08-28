# Channel Watch（/watch 观察者会话）

## 背景

群里的 bot 此前只有被 @ 才醒来：非 mention 消息在 gate 即被丢弃（仅记 passive
receipt），bot 对群里的讨论完全无感。想要的是 bot 作为群的**一个完整在场者**：
读完所有消息，自己判断什么时候该说话——而不是「被 @ 才机械应答」。

## 决策

**watch = ingress 上的一个 tee + 一张状态牌，不是新消息管线。**

- watch-on 群里每条过 access control 的普通消息（**含 @**）原样镜像（steer）进
  该群唯一的常驻观察者会话（mapping key `watch:{chat_id}`），mention 触发的
  对话会话路径整体挂起——**观察者是群里唯一的消息消费者**，它自己决定何时
  回复（经自己 skill 列表里覆盖该平台的 skill，按消息头 `[msg_id: …]` /
  `[thread: …]` 锚定），没有匹配 skill 即纯只读。
- 观察者的 routing 行 `kind ∈ {watch, watch_off}`，在 channel 替 session 说话
  的总出口（事件转发器 → delivery pool）一处挡下全部外发：无状态卡、无
  typing、无投递、无 reaction、无订阅通知。其最终文本只落 session
  transcript（可审计）。
- **mapping 行即开关**：`/watch on` eager 建 session（`kind=watch`）；
  `/watch off` 翻成 `watch_off` 并 cancel 进行中的 run——行、session、上下文
  原地保留，再 on 时 `get_or_create_session` 的 reuse 分支把 kind 翻回、原
  观察者带记忆续任。无需独立开关表。
- 契约不进首条消息（会被 compaction 摘要掉）：spawn 时 conductor 按 routing
  kind 把 `prompt::watch_section` 追加进 system prompt，与 attachments /
  mentions 契约段同机制、免疫压缩。契约核心：@ 你的通常该答（经 skill）；
  其余沉默默认；watch 期间没有别的会话替你答。

被否掉的备选：

- **独立 `channel_watch` 开关表**（初版实现）：开关与 mapping 两个生命周期
  两张表。被「kind 三态」取代——off 只是翻 kind，续任白捡，migration 只剩
  一个 ALTER。代价：开关状态随 session 被 gc 而消失（90 天静默的群 watch
  自动关，需重新 on）——可接受，记为已知边界。
- **mention 仍走对话会话、观察者只看不答**（初版语义）：双 session 分治
  丢失单一在场者的连贯性，且观察者恰好对最重要的消息（@ 它的）不能答。
  被「观察者独收、agent 自判回复」取代（hrli 拍板）。
- **消抖 buffer**：off-stream 状态 + 新失败模式；steer 直进，突发由运行中
  会话的 mailbox 自然吸收。
- **kernel 侧分类打标 / `watch_skill` 配置校验 / 配置文件开关**：判断材料
  原始消息里全有；skill 列表 agent 自知；per-chat 运行时状态足够。

## 行为

- `/watch`（群聊顶层；DM 与话题内拒绝）查状态；`/watch on|off`（admin）
  切换。watch-on 期间已知命令免 @ 执行（gate 放行，命令回复是 hub 自己的
  声音）；off 后恢复 mention 门控（再开要 @）。
- watch-on 群里：普通消息与 @ 一律无 ack、无状态卡、无对话会话，全部
  只进观察者；它说话的唯一形式是 skill 发出的普通消息。命令不受此限。
- `/sessions` 观察者标 👁（含 paused）；`/bind` 拒绝重绑观察者（on/off 同）。
- 镜像内容：消息原始 content（adapter 头含 ts/from/chat/**msg_id**/thread/root）
  + 图片 image_key 文本引用（不下载，要用经 skill 自取）。
- daemon 重启：mapping 行在 sqlite，状态自恢复。
- bot 自己（或其它 bot）经 skill 发出的消息会作为事件回推到观察者——e2e
  实测观察者能识别自回显并保持沉默（防 loop 靠 agent 判断 + blocked_users）。

## 注意点

- **token 量级**：空闲时每条消息一个「看一眼决定说/不说」的 run（prompt
  cache 压住单价）。中低流量群适用；高流量群熔断（每小时 run 上限）留作
  后续。
- **注入面**：触发面 = 群里每句话；观察者以 Dangerous 自动批准 + 平台
  skill 在手（能以 bot 身份发言）。blocked_users 与 access control 是硬
  防线；「channel 不代发」是硬边界，「只用 skill」是 prompt 级约束。
- **gc 边界**：observer session 被 gc（默认 90 天静默）后 mapping 行随之
  删除，watch 静默关闭，需要重新 `/watch on`（独立表方案能避免，被最小化
  取舍否掉）。
- **观察者看不到对话会话的回复**（bot 消息不回推本 bot）——watch-on 期间
  对话会话本就挂起，此洞只在 off 期间存在，可接受。

## 评审决议（三路 review 后修订）

- **悬空 mapping 自愈**：session 被删（手动/gc）而 mapping 残留时，
  `get_or_create_session` 与 tee 快速路都会验证 session 存活、删陈腐行
  重建——否则 watch 群会静默黑洞（mention 已挂起、镜像投向死 session）。
  `Kernel::delete_session` 同步级联删 mapping（治本）。
- **gate→dispatch 单一快照**：watch 状态在 gate 读取一次并随消息穿过
  dispatch 队列；tee 不再二次读取——消除 `/watch` 切换与在途消息的竞态
  （先于 on 到达的 @ 不会被「镜像+触发」双消费）。
- **`/watch off` 排干 mailbox**：翻 kind + cancel 之外清空该 session 的
  steer/queue 队列——已镜像的消息不得在 off 之后唤醒观察者（skill 声音
  不受 kind 门控，只有 channel 投递受）。
- **tee 快速路无写放大**：已有 mapping 直接 steer（每条消息 2 次索引读、
  0 写）；仅缺失/悬空时走加锁的 get-or-create。
- 契约按 kind 分态：paused 观察者拿到「镜像已暂停」变体（投递抑制条款
  保留）；intake 条款改述「every non-command message」（命令从不镜像）。
- 新增测试：forwarder 抑制单点（watch 会话 adapter 零流量 + 对照组正常
  投递）、tee 镜像集成（懒创建/无重复 session/悬空自愈）、`/bind` 拒绝、
  gc watch 边界、gate 快照断言、契约 paused 变体。

## 实现

- `storage/migrations.rs` v24：`channel_session_mappings` 增 `kind` 列
  （`'normal'|'watch'|'watch_off'`）。
- `channels/mod.rs`：`MappingKind` 三态、`SessionRouting.kind` + `is_watch()`
  （Watch|WatchPaused）、`WATCH_KEY_PREFIX` / `watch_mapping_key`；store
  trait 增 `get_watch_state`（按 watch key 查 kind，默认实现 mock 无感）。
- `channels/hub/watch.rs`（新）：tee 本体 `mirror_message`——快速路
  find_mapping + 存活校验直接 steer（零写），缺失/悬空走
  `get_or_create_session`（后者与其共用悬空自愈逻辑）。
- `channels/hub/mod.rs`：dispatch 循环 tee（gate 快照 && `Command::None`）；
  事件转发器 `routing.is_watch() → continue`（投递抑制单点）。
- `channels/hub/gate.rs`：watch-on 群普通消息与 @ 全 `NotAddressed`（静默），
  已知命令放行；watch 快照随消息三元组进 dispatch（单一读取，无 toggle
  竞态）。
- `channels/hub/routing.rs`：`get_or_create_session` 复用前校验 session
  存活，悬空行删除重建（channel 会话通用自愈）。
- `kernel/mod.rs`：`delete_session` 级联删除 channel mappings。
- `channels/hub/command.rs` + `handlers.rs`：`/watch`（admin、chat 级；on
  eager 建、off 翻 kind + cancel + 清 mailbox）；`/bind` 拒绝、`/sessions` 👁。
- `kernel/conductor.rs` + `prompt/mod.rs`：spawn 按 routing kind 追加
  `watch_section` 契约（paused 变体）。
- `platform/feishu_events.rs` / `telegram.rs`：消息头增 `[msg_id: …]`。
