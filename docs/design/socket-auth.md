# Socket 鉴权（ws/wss header 方案）

## 背景

daemon 的 IPC socket 此前完全无鉴权：任何能连上 socket 的进程都能调用全部 RPC（包括跑 agent → 执行 shell）。unix socket 靠文件权限（0600）兜底，但 `ws://`/`tcp://` 一旦绑定到非 localhost 地址即裸奔。

## 决策

**鉴权沉在传输层，不进入 wire 协议。** 每种传输用与其匹配的保护手段：

| 传输 | 保护手段 |
|---|---|
| `unix://` | 文件系统权限（0600），协议无感 |
| `ws://` / `wss://` | WebSocket Upgrade 握手时校验 `Authorization: Bearer <密码>`，不过即 401，连接根本不建立 |
| `tcp://` | **移除**。ws 即 tcp+HTTP 的严格超集；保留一个永远无法鉴权的传输只会留下"配了哈希却不生效"的哑巴亏 |

被否掉的备选：

- **wss URL query 传密码**：可行（同一握手回调解析），但 URL 会进日志/前端/错误消息，泄露面大；header 不落 URL。
- **wire 协议加 `Auth` RPC / `Hello` 带密码**：全传输统一 + 结构化错误，但要动协议、bump 版本、维护连接级门闸状态；且它多覆盖的场景（tcp 裸暴露）正是想劝退的用法。鉴权是传输层的事，不应渗进应用协议。

## 行为

- daemon：config.toml 的 `socket_auth_hash = "blake3:<hex>"` 启用鉴权（`YOMI_SOCKET_AUTH_HASH` 环境变量覆盖，便于临时/容器场景）；仅对 ws listener 生效（unix 忽略）。未设置 = 现状，完全向后兼容。
- 客户端：`YOMI_SOCKET_AUTH=<明文密码>`，`transport::connect` 在 ws/wss 握手自动附加 `Authorization: Bearer` 头；失败报 `socket auth failed: missing or invalid token (set YOMI_SOCKET_AUTH or pass an explicit token)`（`PermissionDenied`）。也可显式传 token（`RemoteKernel::connect_with_auth`；显式值缺失或全空白时回退环境变量），token 存于客户端实例、重连复用；GUI 远程连接弹窗即走此路径（掩码输入，随地址存入 GUI 偏好，明文字段与 config 里的 API key 同级敏感）。
- 校验：`blake3(password)` hex 与配置哈希常量时间比较。定位为高熵机器 token（非人类口令），故不需要慢哈希/盐。
- 爆破防护：握手失败时 401 立即发给客户端，随后 accept 循环固定睡眠 300ms 再接受下一条连接——失败握手在 accept 循环中串行，在线爆破速率全局封顶 ~3 次/秒（与攻击并行度无关；代价是洪泛时正常连接排队，个人 daemon 可接受）。离线防护依赖密码熵：`yomi daemon auth-hash --generate` 生成 128-bit 随机 token（推荐路径），短密码会触发告警。
- 哈希生成：`yomi daemon auth-hash [密码]`（无参从 stdin 读；`--generate` 打印随机 token + 哈希）。
- wire 协议**零改动**（proto 仍为 28）：旧客户端连未启用鉴权的 daemon 完全不受影响。
- 部署形态：跨机器时 yomi 仍只 bind `ws://`，TLS 由反代（caddy/nginx）终结，客户端走 `wss://`；`Authorization` 头标准透传。明文密码只在 ws 裸传时可被嗅探——跨机器请套 TLS。

## 注意点

- supervised 扩展进程继承 daemon 环境：ws 鉴权部署下，本机扩展/工具链（watchdog、doctor、`yomi rpc`）也需导出 `YOMI_SOCKET_AUTH`。

## 实现

- `kernel/src/transport/auth.rs`：`hash_password` / `auth_verifier`（常量时间比较）/ `is_valid_hash_format`（启动期格式校验）。
- `kernel/src/transport/mod.rs`：`Listener::Ws` 持 `Option<AuthVerifier>`，`accept` 用 `accept_hdr_async` 校验；`connect`/`connect_with_token` 附加 header；`SocketAddr::Tcp` 删除（裸 `host:port` 现解析为 ws）。
- `cli daemon start` / gui 内嵌 daemon：`bind` 时从 config（`socket_auth_hash` 字段，`YOMI_SOCKET_AUTH_HASH` 覆盖）构造 verifier；格式非法时 CLI 侧直接报错退出（非零码），GUI 侧打 error 日志并按未启用鉴权启动（进程内 daemon 不能退出整个应用）。
- 测试：`transport/auth_test.rs`（哈希/校验单测 + ws 握手门闸集成测试）。
