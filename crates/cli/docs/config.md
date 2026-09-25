# 配置

## 命令

| 命令 | 作用 |
| --- | --- |
| `yomi config show` | 打印生效配置（含全部默认值） |
| `yomi config get <key>` | 读单值（嵌套 key 如 `agent.default_model`）；不存在即 exit 1 |
| `yomi config set <key> <value>` | 写值（支持嵌套 key，如 `env.SEARXNG_URL`）；**之后必须 `daemon restart` 生效** |
| `yomi config schema` | 打印配置的 JSON Schema（由代码生成，永不漂移；仓库 `docs/config-schema.json` 即其输出，有测试锁定） |

相关：`yomi daemon auth-hash [密码]`——算 socket 密码的 blake3 哈希。省略密码则从 stdin 读（不进 shell history）；`--generate` 生成随机高熵 token，输出明文 token + 哈希 + 配置指引（抗暴力破解全靠 token 熵，推荐）。

## 行为要点

- 配置文件默认 `~/.yomi/config.toml`（`-c/--config` 全局可换）。
- `get`/`set` 按 `.` 分隔走 JSON 对象取值，数组字段（如 `[[models]]`）无法按键索引——改数组请直接编辑配置文件。
- `[env]` 段：启动时注入进程环境且**覆盖宿主同名变量**；改与删都须 `daemon restart` 才生效——重启子进程会先清掉旧注入值再重读 `[env]`。
- GUI 另从 `~/.env`（Windows 为 `%USERPROFILE%\.env`）读环境变量，改后重启 GUI。
- 字段全量参考：仓库 `docs/CONFIG.md`；搜索类 env（SearXNG/Kimi/Serper/Brave）见 `yomi doc websearch`。

## socket 鉴权（ws 远端 attach）

daemon 只听 unix socket 或明文 ws——TLS 靠反代终结，客户端连 `wss://`。鉴权在 ws Upgrade 握手查 `Authorization: Bearer`，不过即 401（映射 PermissionDenied）。

1. daemon 端：`yomi daemon auth-hash --generate`，把输出的哈希写入 `socket_auth_hash`（或 env `YOMI_SOCKET_AUTH_HASH`）。
2. 客户端：env 设 `YOMI_SOCKET_AUTH` 为明文 token。

边界与坑：

- unix socket 不查鉴权（靠文件权限）。
- **ws 监听未配鉴权 = 端口可达即完整 RPC（含 shell 执行）**，daemon 启动会警告。
- `YOMI_EXTRA_SOCKET` 可加第二个监听（如 `ws://0.0.0.0:57231` 供反代），同样按 `socket_auth_hash` 校验。
- `tcp://` 已移除，裸 `host:port` 一律按 ws 解析；Windows 默认 IPC = `ws://127.0.0.1:57231`。
- 设计决策见仓库 `docs/design/socket-auth.md`。
