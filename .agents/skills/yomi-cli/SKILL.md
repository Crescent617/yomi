---
name: yomi-cli
description: "yomi CLI 运维管理速查（daemon、会话管理、gc 清理、配置、用量、cron、事件流）。当需要运维或管理 yomi daemon 及其数据时使用。"
---

# Skill: yomi-cli 运维管理

按 **场景** 查命令。flag 细节一律以 `yomi <cmd> --help` 为准——这里只记什么时候用哪个，以及 help 里看不出来的坑。全局选项 `-c/--config`、`-d/--dir` 所有子命令通用。

## daemon 运维

daemon 是常驻进程：IM 通道（飞书/Telegram）、多客户端共享都跑在它上面。

- `yomi daemon status` / `stop` / `restart`；`start` 仅供内部调用。
- 重启有三条等效路径：`yomi daemon restart`、IM 通道里发 `/restart`（限 `admin_users`）、GUI 改配置后自动重启。会话数据在 sqlite，重启不丢；进行中的 run 会被打断。

## 配置

- `yomi config show` / `get` / `set`：查看与修改配置。
- `set` 之后需 `yomi daemon restart` 生效（没有 reload 命令）。

## 会话管理

- `yomi session list` 默认只列当前目录的会话，`-a` 列全部。
- `session cancel` 停 agent loop（会话保留）；`session stop` 从 daemon 内存移除。
- `yomi session checkpoint cleanup` 清理无属主的 checkpoint 备份文件。

## 清理

- `yomi gc` **默认 dry-run**，`--yes` 才真删；`--days`、`--keep-pinned`、`--sweep-orphans`、`--vacuum` 缺省全部回落 `[gc]` 配置段；`--json` 输出报告。daemon 侧可在 `[gc]` 配 `auto` 每天自动清。清理范围：过期会话 + 无属主文件 + 本地缓存库（cache.db，vacuum 时压缩）。

## 查询与调试

- `yomi run "prompt" [--resume <id>|--last] [--format json|stream-json] [--timeout N]`：headless 一次性运行，等 agent 结束后输出结果并按成败给退出码（0 成功 / 2 失败 / 3 超迭代 / 124 超时）。默认 daemon 活着就走 daemon，否则本地；`--ephemeral` 不记录 last session。脚本里要执行工具记得 `--yolo` 或 `--auto-approve`，否则权限请求会被立即拒绝。
- `yomi usage [-n 天数] [--model X] [--provider Y]`：token 用量统计。
- `yomi events [-s <session>]`：会话事件 NDJSON 流；`--all` 订阅所有会话（仅实时）；`--after-event-id` 断点续传。
- `yomi cron list|get|create|update|pause|resume|delete` 管理定时任务；`cron trigger <id>` 立即手动触发一次。
- `yomi skill list`：列出全部可用 skills（仅全局目录，workspace 的 `.agents/skills` 按会话加载，不在此列）。
