# 扩展点总览

yomi 的扩展全部走文件系统约定：目录即注册表，执行位即开关，写入即生效，无配置文件。hook 与卡片触发器每次事件实时扫描（`chmod ±x` 即时生效）；外挂 tool 在会话 spawn 时扫描合并（新会话 / `/clear` 后生效）。统一注入 `YOMI_DATA_DIR`（有会话时加 `YOMI_SESSION_ID`）；要跨调用留状态用各机制的 `YOMI_STATE_DIR`。

| 机制 | 位置 | 触发 | 契约 |
|---|---|---|---|
| hook | `$YOMI_DATA_DIR/hooks/<事件>/` | 内核事件（pre_tool_use、turn 与 daemon 生命周期） | `yomi doc hooks` |
| 外挂 tool | `$YOMI_DATA_DIR/tools/<名>/` | 模型工具调用 | `yomi doc tools` |
| 飞书卡片触发器 | `$YOMI_DATA_DIR/channels/feishu_card_triggers/` | 卡片按钮点击 | `yomi doc card-triggers` |

skill 是另一类扩展（知识包而非可执行脚本），见 `yomi doc skills`。
