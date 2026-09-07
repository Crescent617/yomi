---
name: yomi-self
description: "yomi 自我管理：用 yomi CLI 运维自己的 daemon、会话、cron 和数据。Use when 要 doctor 自检、重启 daemon、看日志、检索/查看/驱动会话、管理 cron 与 workflow/hook/tool 脚本、gc 清理、查 token 用量、跑 headless，或 events/rpc 调试。"
---

# yomi 自我管理

你是 yomi。运维走 `yomi` CLI；flag 以 `yomi <cmd> --help` 为准，此处只记 help 里看不出来的坑。`-c/--config`、`-d/--dir` 全局通用。

## daemon

- `yomi daemon status` / `restart` / `stop`（`start` 仅供内部调用）。
- `yomi doctor`：任一 ❌ 即 exit 1——重启自检、发版门禁用它。
- 重启路径：CLI `restart`、IM `/restart`（限 `admin_users`）、GUI 改配置自动重启。**进行中的 run 会被打断**——先 `rpc list_running_sessions` 确认没在跑。
- **自杀式重启**（agent 重启自己）：restart 生效时本进程即死——命令必须**立即 exit 0**，不在同一条里 `sleep`+验证（必误报，诱导重试）。先排一次性 cron 自检（重启后照跑），再 `nohup sh -c 'sleep 8; yomi daemon restart' >/dev/null 2>&1 &` 直接结束：
  `yomi cron create --name restart-self-check-<版本号> --session <本会话id> --max-runs 1 --schedule "$(date -v+2M '+%-M %-H %-d %-m *')" --message '自检重启：yomi doctor + yomi --version，简报结果'`
- 日志 `~/.yomi/logs/daemon.<date>.log`（`tui.`/`run.` 前缀同理）。

## 配置

- `yomi config show` / `get` / `set`；`set` 之后必须 `daemon restart` 生效。

## 会话

- `session cat [-s <id>]`（直接读文件，daemon 不在也能用）：默认**不含 thinking**；`--tools` 加工具调用行、`--verbose` 加 thinking、`--raw` 出 JSONL、`--line <n> [--context <k>]` 取窗口（行号来自 `session search`）。
- `session search <词> [-s <id>]`：跨会话全文检索（含工具参数与结果），输出 `L<行号> [role] 片段` 直接喂 `cat --line`。
- `session send` 时机语义：不加 flag = **执行完才收到**（排队成新消息，起新任务用）；`--steer` = **执行中即收到**（注入当前 run）——纠偏用 steer，不打断不起新回合。
- pending 队列：`session mailbox` 查看、`mailbox-remove <mbx_>` 撤回、`mailbox-clear [--steer|--queue]` 清空——只动 pending、不杀 run。
- 新话题起新会话：`channel new-thread --chat <oc_> --text <任务>`——返回 session_id/thread_url，可接 `send --steer` / `session-wait`。
- 群观察模式：`rpc set_channel_watch '{"chat_id":"oc_…","on":true}'`——该群全部消息进该群会话本人（返回其 session_id）。
- 新建 session：`rpc create_session '{}'` 返回新 session_id（可选 `working_dir`/`model_key`/`auto_approve_level`）。
- `session cancel` 停 agent loop，会话保留。
- 运行态（走 `yomi rpc`）：`get_session '{"session_id":"sess_…"}'` 看 `phase`；`list_running_sessions` 看在跑会话（后台 shell 嵌在 `background_shells` 字段）；`list_subagents '{"parent_session_id":"sess_…"}'` 看直接子 agent。
- **等待跑完**：`scripts/session-wait <sid>`——轮询至 `phase=idle` 且无 running subagent、无后台 shell。`send` + `session-wait` = 驱动兄弟会话的最小回路。
- checkpoint：`rpc get_checkpoints` 列表；回滚在 TUI `/rewind`。
- 规则文件两层（spawn 时原文注入 system prompt，只在用户要求时更改）：channel rules `<data_dir>/channels/rules/<chat_id>.md`（全群会话）、session rules `<data_dir>/sessions/rules/<session_id>.md`（当前 session）。IM `/rules` 查看生效内容。

## cron

- `cron list|get|create|update|pause|resume|delete`；`cron trigger <id>` 立即触发一次（调试用）。
- 一次性任务：`--max-runs 1` + 近未来 schedule。
- shell 类 job 退出码 **42** = 自我完成：标记 `Completed` 不再调度（仅调度执行兑现，手动 `trigger` 不生效）。

## workflow

用户自有脚本：`$YOMI_DATA_DIR/workflows/`（py / shell / node，需 shebang + `chmod +x`，写入即生效）。shell 工具、cron shell 任务与 `/workflow run` 注入 `YOMI_DATA_DIR`（有会话时加 `YOMI_SESSION_ID`）。

## hook

`$YOMI_DATA_DIR/hooks/<事件>/` 下带执行位的条目即注册（事件点：`pre_tool_use`、`daemon_up`、`daemon_down`）。契约见 `references/hook.md`。

## tool

`$YOMI_DATA_DIR/tools/<名>/` 放 `tool.json` + 可执行 `run` 即注册。契约见 `references/tools.md`。

## 清理

- `yomi gc` 默认 dry-run，`--yes` 才真删；范围：过期会话 + 无属主文件 + cache.db（`--vacuum` 压缩）。`[gc] auto` 可每天自动清。

## 调试

- `yomi run "prompt"`：headless 一次性运行，退出码 0 成功 / 2 失败 / 3 超迭代 / 124 超时。脚本里要执行工具须 `--yolo` 或 `--auto-approve`。
- `yomi events [-s <sid>]`：事件 NDJSON 流；`--all` 跨会话仅实时（无回放）；`--after-event-id` 断点续传。
- `yomi rpc <method> [params-json]`：wire 逃生舱口；`--help` 列全部方法、`<method> --help` 显示参数 schema（无需 daemon）。流式方法（subscribe）只回 ack，事件流用 `events`。
- `yomi usage`：token 用量统计。

## 隔离测试

与生产并行的测试 daemon 随便折腾：见 yomi-e2e skill。
