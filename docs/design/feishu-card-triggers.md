# 设计文档：feishu_card_triggers —— 用户自定义飞书卡片触发器

渠道层的用户扩展点，与 kernel 接缝上的外挂（`hooks/` `tools/`，见
ext.md）并列：同一 spawn 引擎、同一目录与环境变量约定，注册面在
渠道侧。用户文档见 docs/EXTENSIONS.md「飞书卡片触发器」节。

## 决策记录（2026-09-10，hrli 拍板）

1. 归属 `channels/feishu_card_triggers/`（顶层 `card_triggers/` 被
   否）：回调全生命周期在 channel 层（feishu ws 进、hub 路由、
   adapter 反馈），不碰 agent 回路；平台名入目录——
   `card.action.trigger` 是飞书概念，`PlatformConfig` 已有
   Telegram，第二个平台的交互回调将来平级新增目录，不假装存在
   跨平台 card 抽象。复数目录名，同 `hooks/ tools/ workflows/`
   约定。
2. 单脚本即全部：无 manifest、无 admin 字段（契约只增不改，后加
   零成本）；条目只有带执行位的裸文件一种形态（hooks 的 `<名>/run`
   目录形态不引入——触发器用例就是单脚本，伴生需求用
   `$YOMI_STATE_DIR` 或绝对路径自理）。
3. stdin 提供"足够改卡的信息"（hrli 原话）：`message_id`（任意
   时刻 im API 改卡）+ 回调 `token`（30 分钟内延时更新卡片）两
   路径。`card_sender` 明确不要——实际部署里发卡人恒为 yomi 自己
   的 bot，省掉 click 路径上的一次补拉。凭证不进 stdin，脚本经
   lark-cli / OpenAPI 自理。
4. 回调同步响应不做：飞书回调响应窗口 3 秒，与 30s spawn 超时
   不兼容；维持 ws 帧先 ACK，脚本走 API 改卡（延时更新 token 30
   分钟有效，覆盖 30s 超时有余）。
5. `union_id` 顺带透传（一个字段）；`user_id` 与 contact 查询不
   做——权限相关，脚本自理。
6. 名字长度上限 128（初版 64 照抄 tools 的 provider 上限；触发器名
   不进模型，真实硬边界只有文件名 255 字节——hrli 拍板放宽，
   字符集仍与 tools 同交集）。
7. `ext_` 豁免 channel 用户闸（v0.10.28 后发版首日 hrli 拍板）：
   `blocked_users`/`allowed_users` 不拦触发器点击，权限归脚本自管
   （`operator_open_id` 进 stdin）——白名单不该挡住想服务闸外用户
   的触发器；内建卡片面（审批/mb_/cfg_ 等）闸不变。实现为
   `CardAction::bypasses_user_gate` 单点判断。

## 事件流

```
飞书 ws（card 帧 / event 帧）
 → forward_card_action        # 增提 union_id、token；value 仍 opaque
 → ChannelEvent::CardAction
 → hub 用户闸（ext_ 豁免）      # 权限归脚本自管，见决策 7
 → ns 前缀路由 `ext_` 分支     # 与 mb_*/ask_*/cfg_* 等并列
 → 名字校验                    # 字母开头 [a-zA-Z0-9_-] ≤128，挡路径穿越
 → resolve 注册表              # 带执行位的裸文件，跟随符号链接
 → spawn 引擎                  # 30s、setsid 组杀、stderr ≤64KB
```

未知名/非法名：warn + `send_action_denial` 回 "Unknown card trigger"
（非法名不 echo 原文）。

## stdin 契约（单行 JSON，只增不改）

```json
{"event":"card_trigger","name":"publish","channel":"feishu",
 "operator_open_id":"ou_…","operator_union_id":null,
 "chat_id":"oc_…","message_id":"om_…","token":"c-…",
 "value":{"action":"ext_publish","id":1}}
```

env：`YOMI_EVENT=card_trigger`、`YOMI_DATA_DIR`、
`YOMI_STATE_DIR=state/channels/feishu_card_triggers/<名>/`；
`YOMI_SESSION_ID`、`YOMI_HOOK_EVENT` 显式移除（无会话语义，防
残留）。cwd = 数据目录。

## 退出语义

通知型，无否决：exit 0 成功；非零/超时/spawn 故障只 warn 留痕。
at-least-once，副作用脚本自行幂等。脚本要驱动 agent 走
`yomi session send` 逃生舱（同 signals 删除决策的复用原则）。

## 明确不做

- 回调同步响应体（toast/原地改卡走 response）：3s 窗口不兼容。
- `card_sender` 补拉（决策 3）。
- per-chat 覆盖层、项目级两层合并：等真实需求。
- manifest（admin 门等）：第一个真实需求出现再加。
- 发卡糖命令（`yomi card send`）：lark-cli 已覆盖。
