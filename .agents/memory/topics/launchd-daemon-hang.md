# launchd 下 daemon 启动卡死（2026-08-14）

## 现象

`~/Library/LaunchAgents/com.yomi.daemon.plist`（bootstrap 到 gui/501）启动的 `yomi daemon start`（v0.7.80，brew）**必现卡死**：进程活着、tokio runtime 起了，但永远不绑 socket、不写 daemon.log、不写 pid 文件。终端里 `yomi daemon restart` 起同一个二进制 <1s 就绪。

两次卡死位置不同（都全线程 parked 在 cond_wait）：

1. PID 13165：连 yomi.db/cache.db/tasks.db 都没打开 —— 卡在 `init_kernel` 极早期（config 加载后、storage open 前）。
2. PID 18876（kickstart 重启后）：三个 db 都打开了 —— 卡在 storage open 之后、`init_logging` 之前（log 文件没打开）。

共同边界：都没走到 `init_logging`（daemon.log 无新行、无 log fd）。`sample`/`lldb` attach 在卡死进程上也会超时。

## 环境线索

- launchd 上下文：stdin=/dev/null，stdout/stderr 重定向到 launchd.{out,err}.log，XPC_SERVICE_NAME=com.yomi.daemon，bootstrap 时机器刚从睡眠唤醒（"immediate reason = speculative"）。
- plist 里 EnvironmentVariables 已显式给 HOME + 完整 PATH，排除 PATH 缺工具。
- 排除项：Keychain/密钥子进程（密钥全明文 in config.toml，无子进程）；sqlite 文件锁（无 fd 时卡死说明没到那步）。

## 处置

- `launchctl bootout gui/501/com.yomi.daemon` 卸载 job，改回 CLI spawn（`yomi daemon restart` → detached process，PPID 1）。通道、cron 全恢复。
- plist 文件保留未删，随时可重新 bootstrap。
- **已排除 `XPC_SERVICE_NAME`**：wrapper 脚本（`~/.yomi/bin/daemon-launcher.sh`，保留作记录）unset 后 exec，launchd 下照样卡死（2026-08-14 实测）。
- **现行监管方案**：`~/.yomi/bin/yomi-watchdog.sh`（60s 轮询 `daemon status`，挂了就 restart），由 `~/start.sh` 末尾 `nohup` 拉起（2026-08-14 起），手动跑 `~/start.sh` 即恢复全链路；防重复启动，重复执行安全。注意 `yomi daemon status` 退出码恒 0，只能按输出文本判断（`Daemon is running` / `may be starting up` / `not running`）。**已 E2E 验证**：SIGKILL daemon 后 ~50s 内被自动拉起，stale socket/pid 清理正常，通道恢复（2026-08-14）。

## 待查（下次要搞 launchd 时）

- 在 `daemon start` 入口处（`crates/cli/src/commands/daemon.rs` Start 分支）加最早的 eprintln!/stderr 探针，打 debug build 挂 launchd 复现定位卡点。
- 怀疑方向：launchd bootstrap 上下文（audit session / XPC）里某个懒初始化阻塞——候选 `directories::BaseDirs`、`tokio::fs` 首次驱动 blocking pool、sqlite 首连。
- 注意 CLI spawn 的 daemon **没有开机自启**——重启机器后要手动 `yomi daemon restart`。
