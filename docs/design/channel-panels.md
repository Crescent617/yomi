# 通道面板卡契约（channel panel cards）

飞书通道的卡片分两类，各自一套契约。本文是控制面板卡的约定；
状态卡（run 生命周期卡）的约定见 `channels/render/obs.rs` 模块 doc。

## 两类卡

| 类 | 成员 | 驱动 | 生命周期 |
|---|---|---|---|
| 状态卡 | obs run 卡、ask 决策卡 | run/提问生命周期 | morph/超时自闭合 |
| 控制面板卡 | settings、cron、mailbox、bg、sessions | 用户点击查看/改配置 | 常驻，原地刷新 |

## 控制面板卡契约

1. **点击即执行**：下拉/按钮的回调立即生效（破坏性动作除外——两段
   确认，见 cron delete），无二次确认；执行后
   **原地刷新**（重读状态 → `update_card`），卡片自我解释，不在群
   里另发消息（群里可见的教学文本是命令 ack 的职责，如 `/watch`）。
2. **不自动跟踪变更**：卡片是快照；别处改了配置，点 🔄 Refresh。
3. **标题标真实作用域**：凡在标题里标作用域的卡，必须标操作实际
   生效的范围——`this chat` / `this thread` / `this session` /
   `this channel` / `all chats` / `all sessions`。作用域跟随命令
   落点（thread 内调用 = thread 作用域，与 `/model` 等同规）。
4. **行集合按作用域粒度裁剪**：一个配置项没有某粒度的写入路径，
   就不在该作用域的卡上渲染（例：rit/watch 是 chat-only，thread
   卡不渲染），回调臂对该作用域防御性拒绝。
5. **admin 门**：配置修改限 admin（路由层 user 门限对所有按钮生
   效）；停止类动作（bg 行尾 ⏹）与 `/stop` 同档，不叠加 admin；
   mailbox 撤回/清空与 `/mailbox` 命令同档限 admin。
6. **footer 约定**：全局动作（Reset/Refresh）用 default 边框 small
   按钮放行尾；行内动作用 text 型小按钮。
7. **标志随卡往返**：卡片无法从 chat_id 推回群/私/线程，需要的判
   定（dm/th/scope 键）一律序列化进回调值；缺失一律向保守方向回
   落。这些标志是 UI 保护，不是安全边界（admin 本有 RPC 直达）。

## 作用域分类（命令与面板共用）

- **chat-only**：/watch、/threads、chat 卡的 rit/watch 行——thread
  里拒绝（"use … at top level"）。
- **落点感知**：/model、/settings、/mention、/mailbox、/bind、
  /subscribe——thread 内 = thread 作用域。
- **全局**：/status、/usage、/cron——与落点无关，cron 卡标题标
  `all chats`。

新命令/新面板默认落点感知；只有确实无 thread 粒度的才归
chat-only，且必须在 thread 里给出拒绝文本。

## 现状各卡的作用域

| 卡 | 作用域 | 标题 |
|---|---|---|
| settings | 落点感知（chat / thread） | `⚙️ Settings · this chat` / `· this thread` |
| cron | 全局 | `⏰ Cron jobs · all chats` |
| mailbox | session（落点会话） | `⏳ Pending (n) · this session` |
| bg | session / 全局（--all） | `🖥 Background tasks · this session` / `· all sessions` |
| sessions | channel | `📋 Recent sessions (a–b) · this channel` |
| welcome | —（入群一次性说明卡） | `👋 Hi, I'm yomi` |
