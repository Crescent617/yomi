---
name: yomi-e2e
description: "yomi E2E 隔离测试：用 ~/.yomi/config-test.toml + 独立 socket 起一个与生产并行的测试 daemon。当需要端到端验证 daemon / CLI / 通道行为，或要一个隔离实例随便折腾时使用。"
---

# Skill: yomi-e2e 隔离测试 daemon

测试 daemon = 与生产 daemon 并存的第二个 yomi 实例。隔离全靠两个环境变量：

- `YOMI_CONFIG` → 配置文件（优先级高于默认 `~/.yomi/config.toml`）
- `YOMI_SOCKET` → socket 地址；PID 文件自动取它的兄弟 `.pid` 文件

**每条 yomi 命令都要带这两个变量**——shell 环境不跨调用保留，漏带即打到生产 daemon。daemon 进程本身经 restart 链式派生会继承 env（spawn 只剥 `[env]` 段的 key），所以 daemon 侧起一次就不丢，要手动带的只有 CLI 侧。

## 环境约定

```bash
# 唯一前缀，全文记作 $E2E；实际执行时展开写全
E2E_PREFIX='YOMI_CONFIG=$HOME/.yomi/config-test.toml YOMI_SOCKET=unix:///tmp/yomi-daemon-test.sock'
```

推导产物：socket `/tmp/yomi-daemon-test.sock`，pid `/tmp/yomi-daemon-test.pid`。生产 socket（`~/Library/Application Support/yomi/daemon.sock`）与 pid 完全不受影响。

## 1. 起

```bash
cargo build   # 用被测二进制；已 build 可跳过
YOMI_CONFIG="$HOME/.yomi/config-test.toml" YOMI_SOCKET=unix:///tmp/yomi-daemon-test.sock ./target/debug/yomi daemon restart
```

`restart` 幂等：没在跑 = 直接起。这样起的 daemon 不带 `--auto-exit`，只会被显式 stop。

## 2. 验

```bash
YOMI_CONFIG="$HOME/.yomi/config-test.toml" YOMI_SOCKET=unix:///tmp/yomi-daemon-test.sock ./target/debug/yomi daemon status
ls /tmp/yomi-daemon-test.sock
```

预期：`Daemon is running` 且 socket 文件存在。启动日志找 `Daemon listening on unix:///tmp/yomi-daemon-test.sock`（日志位置见 §坑）。

## 3. 用

所有 CLI 命令加同一前缀即打到测试 daemon：

```bash
… ./target/debug/yomi session list -a
… ./target/debug/yomi events --all          # 订阅测试 daemon 实时事件流
… ./target/debug/yomi run "prompt" --yolo   # daemon 活着就走 daemon
… ./target/debug/yomi cron list
```

GUI 同样读 `YOMI_SOCKET`：带该变量启动 GUI 即连测试 daemon。飞书通道的触发/验证手法见 feishu-e2e skill（其 §2 读取 config.toml 的地方换成 config-test.toml，日志/db 路径按 §坑调整）。

## 4. 拆

```bash
YOMI_CONFIG="$HOME/.yomi/config-test.toml" YOMI_SOCKET=unix:///tmp/yomi-daemon-test.sock ./target/debug/yomi daemon stop
```

graceful stop 会自己删 socket 和 pid 文件，无残留。

## 坑

| 坑 | 事实与对策 |
|---|---|
| data_dir 未隔离 | config-test.toml 当前**没设** `data_dir` → 与生产共享 `~/.yomi`：同一个 `logs/daemon.<date>.log`（两进程日志交织）、同一个 `yomi.db`/sessions，且 **cron 任务两个 daemon 都会触发**（tasks.db 无选主）。完全隔离：config-test.toml 加一行 `data_dir = "~/.yomi-test"`（log_dir 默认跟随），测试日志即落在 `~/.yomi-test/logs/`。也可用 `YOMI_DATA_DIR` env，但同样要每条命令都带 |
| env 里别写 `~` | 用 `$HOME`；`env A=~/x cmd` 形式下 `~` 不展开 |
| 交互式 UI 慎入 | `yomi`（TUI）带前缀也能连测试 daemon，但 TUI 会在当前终端占屏，E2E 脚本化优先用 `run` / `events` / `session send` |
