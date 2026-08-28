---
name: yomi-self
description: "yomi 自我管理：用 yomi CLI 运维自己的 daemon、会话、cron 和数据。Use when 要健康自检（doctor）、检查/重启 daemon、看日志、检索或查看会话（search/cat/list/send/cancel/等待跑完）、管理 cron、管理 workflow 脚本、gc 清理、查 token 用量、跑 headless 任务，或用 events/rpc 调试。"
---

# yomi 自我管理

你是 yomi。这个 skill 是你管理自己运行时的手册：daemon、会话、cron、数据，全部通过 `yomi` CLI 完成。flag 细节一律以 `yomi <cmd> --help` 为准——这里只记**场景 → 命令**的映射，以及 help 里看不出来的坑。全局选项 `-c/--config`、`-d/--dir` 所有子命令通用。

内核三态（仅 `run`/`tui`）：默认 auto——活 daemon 且 hello 握手通过就走 daemon，没有 daemon 才落本地 in-process；daemon 活着但 hello 失败会直接报错，不静默 fallback。`--bg` 强制 daemon（不在则 spawn）；`--fg` 强制本地。

## daemon（自己的生命周期）

常驻进程：IM 通道（飞书/Telegram）、多客户端共享都跑在它上面。

- `yomi daemon status` / `restart` / `stop`；`start` 仅供内部调用。
- `yomi doctor`：健康自检（config / daemon 握手含协议版本 / 渠道连通 / cron / storage），任一 ❌ 即 exit 1——重启自检、发版门禁用它。
- 重启等效路径：CLI `restart`、IM 通道 `/restart`（限 `admin_users`）、GUI 改配置自动重启。会话数据在 sqlite，重启不丢；**进行中的 run 会被打断**——重启前先 `rpc list_running_sessions` 确认没在跑。
- **自杀式重启**（agent 在 daemon 里重启自己）：命令必须**立即 exit 0**，绝不在同一条里 `sleep`+验证——restart 生效时本进程即死，后续验证必然以"失败"误报，诱导重试跑两遍。正确姿势：先排**一次性 cron 自检**（job 持久化在 sqlite，重启后照跑），再 `nohup sh -c 'sleep 8; yomi daemon restart' >/dev/null 2>&1 &` 直接结束：
  `yomi cron create --name restart-self-check-<版本号> --session <本会话id> --max-runs 1 --schedule "$(date -v+2M '+%-M %-H %-d %-m *')" --message '自检重启：yomi doctor + yomi --version，简报结果'`
  （name 带版本号：同名 create 幂等返回旧任务，不更新。）
- 日志在 `~/.yomi/logs/daemon.<date>.log`（`tui.`/`run.` 前缀同理）——行为异常先看这里。
- `session`/`cron`/`events`/`rpc` 等 daemon-only 命令**不会自动 spawn daemon**，连不上即报 "Is it running?"。

## 配置

- `yomi config show` / `get` / `set`；`set` 之后必须 `daemon restart` 生效

## 会话（自己或兄弟会话）

- `session list` 默认全列，`-d` 按目录过滤。
- `session cat [-s <id>]` 读会话消息日志（直接读文件，不依赖 daemon）：默认友好输出（user/assistant 文本 + 图片 asset 路径，**不含 thinking**）；`--tools` 加工具调用行；`--verbose` 加 thinking 块；`--raw` 输出 JSONL；`--line <n> [--context <k>]` 按行号取窗口，行号来自 `session search`。
- `session search <词> [-s <id>] [--json] [--verbose]`：跨会话全文检索（含工具参数与结果，thinking 仅 `--verbose` 纳入），按会话分组输出 `L<行号> [role] 片段`，行号直接喂 `cat --line`。
- `session send` 往会话注消息，时机语义不同：不加 flag = **执行完才收到**（排队成新用户消息，起新任务用它）；`--steer` = **执行中即收到**（注入当前 run，回合间生效）——补充信息、中途纠偏用 steer，不打断也不另起回合。
- pending 队列管理：`session mailbox` 查看，`session mailbox-remove <mbx_>` 撤回单条，`session mailbox-clear [--steer|--queue]` 按队列清空——只动 pending、不杀 run（区别于 cancel）。前端经 rpc（mailbox_snapshot / remove / clear）管理，`mailbox_changed` 事件（附双队列计数）触发刷新。
- 新话题起新会话干活：`channel new-thread --chat <oc_> --text <任务>`——话题里的后续发言进同一会话；返回 session_id/thread_url，可接 `send --steer` / `session-wait`。`--channel` 选填，仅同平台多通道时消歧用。
- 新建 session：`rpc create_session '{}'` 返回新 session_id（可选 `working_dir`/`model_key`/`auto_approve_level`，缺省继承配置）。
- `session cancel` 停 agent loop，会话保留。
- 观察运行态（都走 `yomi rpc`）：
  - `get_session '{"session_id":"sess_…"}'`：单会话 `phase`（idle/streaming/executing_tool/compacting）。
  - `list_running_sessions`：在跑会话（有后台任务的 idle 会话也在列）；后台 shell 任务嵌在 `background_shells` 字段（task_id/pid/command/output_path/started_at），无独立 rpc。
  - `list_subagents '{"parent_session_id":"sess_…"}'`：直接子 agent（`is_running`）；会话不存在返回空数组而非报错。
- **等待跑完**：`scripts/session-wait <session_id>`——轮询（无超时）至 `phase=idle` 且无 running subagent、无后台 shell；退出码 0 安静 / 2 用法错或首查失败。`session send` + `session-wait` = 驱动兄弟会话干活并等它完成的最小回路。
- checkpoint：列表走 `rpc get_checkpoints`；回滚在 TUI `/rewind`；无属主备份由 `gc` 孤儿 sweep 清理。
- RULE.md：`<data_dir>/sessions/rules/<sid>.md` 非空即在 spawn 时原文注入该会话的 system prompt——改动**下次 spawn 生效**（agent 空闲约 2 分钟卸载后），运行中的 agent 看不到；除非用户要求，不要轻易修改。

## cron（自己的闹钟）

- `cron list|get|create|update|pause|resume|delete`；`cron trigger <id>` 立即手动触发一次，调试任务时用。
- 一次性任务（如重启自检，见 daemon 节）：`--max-runs 1` + 近未来的 schedule。
- shell 类 job 脚本退出码 **42** = 自我完成：标记 `Completed` 不再调度（仅调度执行兑现，手动 `trigger` 不生效）。

## workflow（全局脚本）

用户自有的可执行脚本：`$YOMI_DATA_DIR/workflows/`（py / shell / node 均可，需 shebang + `chmod +x`，写入即生效）。shell 工具、cron shell 任务与 `/workflow run` 都会注入 `YOMI_DATA_DIR`（及有会话时的 `YOMI_SESSION_ID`），脚本里用 `"$YOMI_DATA_DIR"` 定位 yomi 数据目录。

## 清理（自己的数据）

- `yomi gc` **默认 dry-run**，`--yes` 才真删；不带参数时缺省值回落 `[gc]` 配置段。清理范围：过期会话 + 无属主文件 + cache.db（`--vacuum` 时压缩）。daemon 侧 `[gc] auto` 可每天自动清。

## 调试

- `yomi run "prompt"`：headless 一次性运行，退出码 0 成功 / 2 失败 / 3 超迭代 / 124 超时。脚本里要执行工具须 `--yolo` 或 `--auto-approve`，否则权限请求被立即拒绝。
- `yomi events [-s <sid>]`：会话事件 NDJSON 流；`--all` 跨会话但仅实时（无回放）；`--after-event-id` 断点续传。
- `yomi rpc <method> [params-json]`：wire 协议逃生舱口，任意 `ReqMethod` 直打 daemon，result 以 JSON 输出；参数可经 stdin 传入。`--help` 列全部方法、`<method> --help` 显示参数 schema（无需 daemon）。流式方法（subscribe）只回 ack，事件流用 `events`。
- `yomi usage`：token 用量统计。

## 隔离测试

要一个与生产并行的测试 daemon 随便折腾（`YOMI_CONFIG` + `YOMI_SOCKET` 两环境变量隔离）：见 yomi-e2e skill。
