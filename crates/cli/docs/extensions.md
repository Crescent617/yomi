# 扩展点总览

yomi 的扩展全部走文件系统约定：目录即注册表，执行位即开关，写入即生效，无配置文件。hook 与卡片触发器每次事件实时扫描（`chmod ±x` 即时生效）；外挂 tool 在会话 spawn 时扫描合并（新会话 / `/clear` 后生效）。统一注入 `YOMI_DATA_DIR`（有会话时加 `YOMI_SESSION_ID`）；要跨调用留状态用各机制的 `YOMI_STATE_DIR`。spawn 型外挂（hook/tool/卡片触发器）的 stdin JSON 带 `v` 契约版本字段（当前 1，只增不改）。

**扩展包**（`yomi doc extension`）是这些注册表之上的安装与生命周期层：GitHub 仓（`owner/repo[/子目录][@ref]`）或本地目录 + `ext.toml` 把 cron/hooks/bin/snippets 作为一个东西 `yomi extension install|list|remove`，统一实体复制、重装即更新，运行时语义不变。

| 机制 | 位置 | 触发 | 契约 |
|---|---|---|---|
| hook | `$YOMI_DATA_DIR/hooks/<事件>/` | 内核事件（pre_tool_use、turn 与 daemon 生命周期） | `yomi doc hooks` |
| 外挂 tool | `$YOMI_DATA_DIR/tools/<名>/` | 模型工具调用 | `yomi doc tools` |
| 飞书卡片触发器 | `$YOMI_DATA_DIR/channels/feishu_card_triggers/` | 卡片按钮点击 | `yomi doc card-triggers` |
| 扩展包 | 源目录 + `ext.toml` | `yomi extension install` | `yomi doc extension` |
| ext 路由 | 无目录，`yomi rpc route_session` | 外部系统按键路由会话（与渠道会话同一映射收口） | `yomi doc sessions` |

skill 是另一类扩展（知识包而非可执行脚本），见 `yomi doc skills`。
