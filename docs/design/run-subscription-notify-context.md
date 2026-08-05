# 订阅完成通知卡附加上下文（引用触发消息）

## 1. 背景

`/subscribe` 的 run 完成通知卡只有一行状态文案：

```
✅ 你订阅的「lhr test group」任务完成 · 查看回复 →
```

订阅多个群/话题、或同一话题多次运行时，通知之间无法区分——必须点跳转才知道是哪一次触发。需要在卡面带上一小段被围观 run 的上下文（触发消息之类），一眼可辨。

方案：卡面加一行 **markdown 引用**，内容 = 该 run 触发消息的原文摘要（能拿到作者名时带上作者；拿不到时回退会话标题，无作者）。

作者名**尽力而为**：新增适配层方法 `fetch_user_name(open_id)`（Feishu 走 `contact/v3/users`，默认实现返回 `None`）。bot 当前无 contact 名字段权限（实测 code 0 但无 `name`）——有权限的部署显示真名，没权限自动省略，不用 `<at>` 标签凑数（DM 里 at 第三方渲染不可控，且可能打扰作者）。

## 2. 现状

- 通知卡由 `hub.rs::subscription_notify_card` 渲染：单行 notation markdown + 整卡 `card_link` 跳转；无 link 时降级为纯文本 ping。DM 给每个订阅者一张（去重），或按目标群各发一张（@ 订阅者）。
- `notify_run_subscribers` 入参：`store` / `adapter` / `routing` / `reply_msg_id` / `status`。两处调用点（forwarder 的 run 结束分支、watchdog 死会话兜底）都在 forwarder 内，`session_id` 与 `kernel: Weak<Kernel>` 均在作用域。
- 可用数据源：
  - `routing.mapping_key`：thread 会话 = 话题根消息 id（`reply_in_thread` 下通常就是触发消息本身）；chat 级会话 = chat id（与 `external_chat_id` 相同）。
  - `adapter.fetch_message(msg_id)` → `HistoryMessage.text`：Feishu 已实现；Telegram 用默认实现（`Ok(None)`）。
  - `kernel.get_session(sid).title`：sqlite 点查。标题创建时取触发文本，之后可能被模型改写为摘要——两种形态都具辨识度。

## 3. 设计

### 3.1 引用来源回退链

```
obs.last_user_msg(sid)（settle ✅ 反应踩的同一条——session 最近一条用户消息）
    → fetch_message 成功且有文本 → 用该消息原文；
        作者名 = fetch_user_name(sender_id)（best-effort，None 则省略）
否则 thread 会话（mapping_key ≠ external_chat_id）:
    fetch_message(mapping_key) → 用根消息原文（hub 重启后 obs 为空时兜底）
否则:
    kernel.get_session(sid).title 非空 → 用会话标题（无作者）
否则:
    不加引用行（保持现状）
```

- 与 settle 反应同源：引用行和 ✅ 永远指向同一条消息——典型 RIT 流程下就是触发消息本身；手动话题里是 @bot 那条（而非他人根消息）；mid-run 追问时是追问那条（也是最终回复所应答的消息）。
- `last_user_msg` 跨 run 粘滞：无新触发的 run（goal 续跑、cron 触发、API steer）会引用上一条用户消息——与 ✅ 反应的目标语义完全一致，接受。
- 仅在 `subs` 非空后才取上下文：每次有订阅的 run 结束最多两次平台 API 调用（消息 + 用户名）+ 一次 sqlite 点查，量可忽略。
- DM 与群目标卡共用同一段引用文本（取一次，传入渲染）。
- `fetch_message` 失败（消息被删、权限、平台不支持）静默落到下一级，不打 warn 以外的副作用。

### 3.2 文本规范化

- 剥离开头的提及占位（`@_user_1` 等，DM 卡里无意义）；
- 压平所有空白与换行（引用行单行）；
- 按字符截断 50，超出加 `…`（复用 `truncate_by_chars`）；
- 非文本消息的 `[图片]` 类占位文本原样使用，不特殊处理；
- 不做 markdown 转义——与现有 chat_name 注入同一风险级，接受。

### 3.3 卡面

引用作为**独立的第二个 markdown 元素**（notation），不并入状态行——`<font>` 标签包裹会破坏行首 `>` 语法，灰色层次交给引用条本身的样式：

```json
{ "tag": "markdown", "text_size": "notation", "content": "> 请用 shell 执行：sleep 35 && echo 任务完成…" }
```

效果：

```
✅ 你订阅的「lhr test group」任务完成 · 查看回复 →
> 李华儒：请用 shell 执行：sleep 35 && echo 任务完成…
```

- 作者名解析成功时前缀 `{名字}：`（全角冒号），否则纯摘要；标题回退分支永远无前缀。
- `card_link`、提及、降级卡结构均不变；降级卡（无 link）同样加引用行。
- 状态行不变；`Completed` / `Failed` 都带引用（`Cancelled` 本就不通知，保持）。

### 3.4 非目标

- 不加按钮、不改跳转交互；不引用 bot 的回复内容（回复有点 jump 直达）。
- 不为 Telegram 补 `fetch_message`——默认 `None` 自动落到标题回退。
- 截断长度固定 50 字符，不引入配置项。

## 4. 变更点

| 文件 | 改动 |
|------|------|
| `channels/mod.rs` | `PlatformAdapter` 新增 `fetch_user_name(open_id) -> Option<String>`（默认 `None`） |
| `channels/feishu.rs` | 实现 `fetch_user_name`（`contact/v3/users`，无 `name` 字段或报错即 `None`，静默） |
| `channels/hub.rs` | `notify_run_subscribers` 加 `session_id` / `kernel` / `obs` 参数（两调用点就地传入），subs 非空后按 §3.1 取 quote 与作者名；`subscription_notify_card` 加 `quote: Option<&str>` 参数与第二个 markdown 元素；新增规范化辅助函数 |
| `channels/obs.rs` | `ObsTracker::last_user_msg_id` getter（复用 settle 反应目标） |

## 5. 测试

- `subscription_notify_card`：有/无 quote、有/无作者名的结构差异；截断与 `…`；`@_user_N` 剥离；换行压平。
- `notify_run_subscribers`（沿用 hub_test 的 mock store/adapter）：引用 settle 反应目标消息（优先于根消息）；作者名解析成功带前缀、`None` 省略；fetch 失败回退根消息/会话标题；全失败则无引用行（现状回归）；chat 级会话跳过消息 fetch 走标题。
- 取消的 run 仍不通知（回归保护）。
