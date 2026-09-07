---
name: yomi-self
description: "yomi 自我管理：用 yomi CLI 运维自己的 daemon、会话、cron 和数据。Use when 要 doctor 自检、重启 daemon、看日志、检索/查看/驱动会话、管理 cron 与 workflow/hook/tool 脚本、gc 清理、查 token 用量、跑 headless，或 events/rpc 调试。"
---

# yomi 自我管理

你是 yomi。全部运维走 `yomi` CLI；flag 以 `yomi <cmd> --help` 为准，这里只记**场景 → 命令**和 help 里看不出来的坑。全局选项 `-c/--config`、`-d/--dir` 通用。

## daemon

- `yomi daemon status` / `restart` / `stop`（`start` 仅供内部调用）。
- `yomi doctor`：健康自检，任一 ❌ 即 exit 1——重启自检、发版门禁用它。
- 重启路径：CLI `restart`、IM `/restart`（限 `admin_users`）、GUI 改配置自动重启。**进行中的 run 会被打断**——先 `rpc list_running_sessions` 确认没在跑。
- **自杀式重启**（agent 重启自己）：命令必须**立即 exit 0**，绝不在同一条里 `sleep`+验证——restart 生效时本进程即死，验证必以"失败"误报，诱导重试跑两遍。姿势：先排一次性 cron 自检（job 持久化在 sqlite，重启后照跑），再 `nohup sh -c 'sleep 8; yomi daemon restart' >/dev/null 2>&1 &` 直接结束：
  `yomi cron create --name restart-self-check-<版本号> --session <本会话id> --max-runs 1 --schedule "$(date -v+2M '+%-M %-H %-d %-m *')" --message '自检重启：yomi doctor + yomi --version，简报结果'`
  （同名 create 幂等返回旧任务，name 带版本号区分。）
- 日志 `~/.yomi/logs/daemon.<date>.log`（`tui.`/`run.` 前缀同理）——行为异常先看这里。
- daemon-only 命令（`session`/`cron`/`events`/`rpc`）不会自动拉起 daemon，连不上即报 "Is it running?"。

## 配置

- `yomi config show` / `get` / `set`；`set` 之后必须 `daemon restart` 生效。

## 会话

- `session cat [-s <id>]` 读消息日志（直接读文件，不依赖 daemon）：默认**不含 thinking**；`--tools` 加工具调用行、`--verbose` 加 thinking、`--raw` 出 JSONL、`--line <n> [--context <k>]` 取窗口（行号来自 `session search`）。
- `session search <词> [-s <id>]`：跨会话全文检索（含工具参数与结果），输出 `L<行号> [role] 片段` 直接喂 `cat --line`。
- `session send` 时机语义：不加 flag = **执行完才收到**（排队成新消息，起新任务用）；`--steer` = **执行中即收到**（注入当前 run）——纠偏用 steer，不打断不起新回合。
- pending 队列：`session mailbox` 查看、`mailbox-remove <mbx_>` 撤回、`mailbox-clear [--steer|--queue]` 清空——只动 pending、不杀 run。
- 新话题起新会话：`channel new-thread --chat <oc_> --text <任务>`——返回 session_id/thread_url，可接 `send --steer` / `session-wait`。
- 群观察模式：`rpc set_channel_watch '{"chat_id":"oc_…","on":true}'`——该群全部消息进该群会话本人（返回其 session_id），设计见 docs/design/watch.md。
- 新建 session：`rpc create_session '{}'` 返回新 session_id（可选 `working_dir`/`model_key`/`auto_approve_level`）。
- `session cancel` 停 agent loop，会话保留。
- 运行态（走 `yomi rpc`）：`get_session '{"session_id":"sess_…"}'` 看 `phase`；`list_running_sessions` 看在跑会话（后台 shell 嵌在 `background_shells` 字段）；`list_subagents '{"parent_session_id":"sess_…"}'` 看直接子 agent。
- **等待跑完**：`scripts/session-wait <sid>`——轮询至 `phase=idle` 且无 running subagent、无后台 shell。`send` + `session-wait` = 驱动兄弟会话的最小回路。
- checkpoint：`rpc get_checkpoints` 列表；回滚在 TUI `/rewind`。
- 规则文件两层（spawn 时原文注入 system prompt，只在用户要求时更改）：channel rules `<data_dir>/channels/rules/<chat_id>.md`（全群会话）、session rules `<data_dir>/sessions/rules/<session_id>.md`（当前 session）。IM `/rules` 查看生效内容（与注入同一读取路径）。

## cron

- `cron list|get|create|update|pause|resume|delete`；`cron trigger <id>` 立即触发一次（调试用）。
- 一次性任务：`--max-runs 1` + 近未来 schedule。
- shell 类 job 退出码 **42** = 自我完成：标记 `Completed` 不再调度（仅调度执行兑现，手动 `trigger` 不生效）。

## workflow

用户自有脚本：`$YOMI_DATA_DIR/workflows/`（py / shell / node，需 shebang + `chmod +x`，写入即生效）。shell 工具、cron shell 任务与 `/workflow run` 注入 `YOMI_DATA_DIR`（有会话时加 `YOMI_SESSION_ID`）。

## hook（事件闸与生命周期）

`$YOMI_DATA_DIR/hooks/<事件>/` 下的条目即注册（带执行位的裸文件，或含可执行 `run` 的目录；按条目名字典序串行，`chmod ±x` 即时生效，无 reload）。事件点：`pre_tool_use`（工具调用前、权限审批前；`0`=放行、`2`=否决（stderr 以 `[hook:<条目名>]` 前缀回流给 agent）、其他非零/超时（30s）=故障放行 fail-open）；`daemon_up` / `daemon_down`（daemon 就绪后 / 关停前，通知型无否决——up 后台跑不占启动、down 等完再拆；其他进程随 yomi 启停用这对，脚本快去快回，常驻进程 `nohup … &` 放后台）。写 hook 脚本前读 `references/hook.md`（stdin schema / env / 示例 / 与 Claude Code 差异）。

## tool（自定义工具）

`$YOMI_DATA_DIR/tools/<工具名>/` 放 `tool.json`（`desc`/`schema`/`level` 缺省 `caution`/`timeout_secs` 缺省 60 上限 600）+ 可执行 `run` 即注册，agent 新会话即可调用；stdin 收 JSON（内含 `args`），exit 0 的 stdout 作结果，非零/超时把 stderr 以 `[ext:<名>]` 前缀报错给 agent（fail-closed）。写 tool 前读 `references/tools.md`（manifest 字段 / 调用契约 / env）。

## 清理

- `yomi gc` 默认 dry-run，`--yes` 才真删；范围：过期会话 + 无属主文件 + cache.db（`--vacuum` 压缩）。`[gc] auto` 可每天自动清。

## 调试

- `yomi run "prompt"`：headless 一次性运行，退出码 0 成功 / 2 失败 / 3 超迭代 / 124 超时。脚本里要执行工具须 `--yolo` 或 `--auto-approve`。
- `yomi events [-s <sid>]`：事件 NDJSON 流；`--all` 跨会话仅实时（无回放）；`--after-event-id` 断点续传。
- `yomi rpc <method> [params-json]`：wire 逃生舱口；`--help` 列全部方法、`<method> --help` 显示参数 schema（无需 daemon）。流式方法（subscribe）只回 ack，事件流用 `events`。
- `yomi usage`：token 用量统计。

## 隔离测试

与生产并行的测试 daemon 随便折腾：见 yomi-e2e skill。
