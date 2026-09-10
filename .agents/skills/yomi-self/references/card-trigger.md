# 飞书卡片触发器契约

渠道侧注册表，按名路由：一个按钮一个处理器。执行位开关、符号链接
跟随与 hook 同约定。

## 目录布局

```
$YOMI_DATA_DIR/channels/feishu_card_triggers/
├── mark_done                        # 带执行位即生效
└── publish -> /opt/triggers/publish # 跟随符号链接（stow/nix 部署）
```

## 触发与路由

卡片按钮的 value 写 `{"action":"ext_<名>", ...}`，用户点击即执行对应
条目。`ext_` 是预留命名空间（内建：`ask_ mb_ act_ bg_ pg_ cfg_
cron_`；无前缀归审批）。名字约束：字母开头、仅 `[a-zA-Z0-9_-]`、
≤64——名字拼进文件路径，此校验挡路径穿越。点击未注册/非法名字回
一条 "Unknown card trigger"（非法名不 echo 原文）。无 reload：每次
点击实时解析，`chmod ±x` 即时生效。

发卡不用 yomi 参与：同一 bot 任意方式发卡（lark-cli、OpenAPI），
回调按应用投递回 daemon。value 其余字段由发卡方自定义，kernel 不
解读、全文透传。

## stdin schema

单行 compact JSON（`snake_case`，稳定契约只增不改）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `event` | string | 恒 `"card_trigger"` |
| `name` | string | 触发器名（`ext_` 已剥离） |
| `channel` | string | channel 名（多 channel 时分辨来源） |
| `operator_open_id` | string | 点击者 open_id（已过 channel 用户闸） |
| `operator_union_id` | string\|null | 点击者 union_id |
| `chat_id` | string\|null | 回调所在聊天 |
| `message_id` | string\|null | 卡片消息 id（改卡定位） |
| `token` | string\|null | 回调 token（延时更新卡片，30 分钟有效） |
| `value` | object | 按钮 value 全文（含 `action` 键本身） |

## 环境变量

| env | 值 | 用途 |
|---|---|---|
| `YOMI_EVENT` | `card_trigger` | 一脚本多用途时分辨 |
| `YOMI_DATA_DIR` | 数据目录 | 定位 yomi 资产 |
| `YOMI_STATE_DIR` | `<data_dir>/state/channels/feishu_card_triggers/<名>/` | 持久状态目录，daemon 惰性创建 |

无会话语义：`YOMI_SESSION_ID` 显式移除；进程 cwd = 数据目录。要
驱动 agent 走 `yomi session send`（sid 由发卡方自行塞进 value）。

## 改卡

| 路径 | 条件 | 接口 |
|---|---|---|
| 回调 token | 点击后 30 分钟内 | 延时更新消息卡片（token 即本次交互授权） |
| message_id | 任意时刻，卡未撤回 | `PATCH /open-apis/im/v1/messages/<id>` |

message_id 路径的发送者规则：**调用身份必须是消息发送者**——bot
发的卡只能 `--as bot` 改，且 lark-cli profile 必须指向发卡的那个
app（多 profile 环境 `lark-cli --profile <名> api …` 显式选）。
凭证不进 stdin，脚本自行经 lark-cli / OpenAPI 调用。

## 退出码

通知型无否决：`0` 成功；非零 / 超时 30s / spawn 失败只记 warn 日志
（stderr 随日志，捕获上限 64KB）。at-least-once：重投或恢复后同一
点击可能再执行，有副作用的脚本自行幂等。
