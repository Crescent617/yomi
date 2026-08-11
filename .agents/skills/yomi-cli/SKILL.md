---
name: yomi-cli
description: "yomi CLI 运维速查：daemon 生命周期、会话管理、cron 定时任务、gc 清理、配置、token 用量、事件流、rpc 调试。当需要运维、调试或管理 yomi daemon 及其数据时使用。"
---

# yomi CLI 运维速查

flag 细节一律以 `yomi <cmd> --help` 为准——这里只记**场景 → 命令**的映射，以及 help 里看不出来的坑。全局选项 `-c/--config`、`-d/--dir` 所有子命令通用。

内核三态（仅 `run`/`tui`）：默认 auto——活 daemon 且 hello 握手通过就走 daemon，没有 daemon 才落本地 in-process；daemon 活着但 hello 失败会直接报错，不静默 fallback。`--bg` 强制 daemon（不在则 spawn）；`--fg` 强制本地。

## daemon

常驻进程：IM 通道（飞书/Telegram）、多客户端共享都跑在它上面。

- `yomi daemon status` / `restart` / `stop`；`start` 仅供内部调用。
- 重启等效路径：CLI `restart`、IM 通道 `/restart`（限 `admin_users`）、GUI 改配置自动重启。会话数据在 sqlite，重启不丢；进行中的 run 会被打断。
- 日志在 `~/.yomi/logs/daemon.<date>.log`（`tui.`/`run.` 前缀同理）——daemon 行为异常先看这里。
- session/cron/events/rpc 等 daemon-only 命令**不会自动 spawn daemon**，连不上即报 "Is it running?"。

## 配置

- `yomi config show` / `get` / `set`；`set` 之后必须 `daemon restart` 生效（没有 reload 命令）。

## 会话

- `session list` 默认只列当前目录的会话，`-a` 列全部。
- `session cat [-s <id>]` 查看会话消息日志：默认友好输出（user/assistant 文本，图片显示 asset 真实文件路径）；`--tools` 加上工具调用行（名称/args/结果，超长截断）；`--raw` 输出 JSONL。直接读文件，不依赖 daemon。
- `cancel` 停 agent loop（会话保留）；`stop` 从 daemon 内存移除。
- `session send` 往会话注消息（agent 忙则排队）；`--steer` 改为注入当前 run，回合间生效。
- `checkpoint rewind` 只展示影响不执行（实际回滚在 TUI）；`cleanup` 清无属主的 checkpoint 备份文件。

## cron

- `cron list|get|create|update|pause|resume|delete`；`cron trigger <id>` 立即手动触发一次，调试任务时用。

## 清理

- `yomi gc` **默认 dry-run**，`--yes` 才真删；不带参数时缺省值回落 `[gc]` 配置段。清理范围：过期会话 + 无属主文件 + cache.db（`--vacuum` 时压缩）。daemon 侧 `[gc] auto` 可每天自动清。

## 调试

- `yomi run "prompt"`：headless 一次性运行，退出码 0 成功 / 2 失败 / 3 超迭代 / 124 超时。脚本里要执行工具须 `--yolo` 或 `--auto-approve`，否则权限请求被立即拒绝。
- `yomi events [-s <sid>]`：会话事件 NDJSON 流；`--all` 跨会话但仅实时（无回放）；`--after-event-id` 断点续传。
- `yomi rpc <method> [params-json]`：wire 协议逃生舱口，任意 `ReqMethod` 直打 daemon，result 以 JSON 输出；参数可经 stdin 传入。`--help` 列全部方法、`<method> --help` 显示参数 schema（无需 daemon）。流式方法（subscribe）只回 ack，事件流用 `events`。
- 观察运行态（都走 `yomi rpc`）：
  - `list_running_sessions`：在跑会话（有后台任务的 idle 会话也在列）。
  - `get_session '{"session_id":"sess_…"}'`：单个会话状态（`phase`: idle/streaming/executing_tool/compacting）。
  - `list_subagents '{"parent_session_id":"sess_…"}'`：某会话的 subagent（会话不存在返回空数组而非报错）。
  - bg shell 任务：无独立 rpc，嵌在 `list_running_sessions` 的 `background_shells` 字段（task_id/pid/command/output_path/started_at）。
- `yomi usage`：token 用量统计。

## 隔离测试

要一个与生产并行的测试 daemon 随便折腾（`YOMI_CONFIG` + `YOMI_SOCKET` 两环境变量隔离）：见 yomi-e2e skill。
