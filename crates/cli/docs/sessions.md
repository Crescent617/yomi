# 会话

检索、查看、驱动会话；运行态查询；等待跑完。

- `yomi session list`：全部会话（`-d/--dir` 按工作目录过滤）。
- `yomi session cat [-s <id>]`：读消息记录（直接读文件，daemon 不在也能用）。默认**不含 thinking**；`--tools` 加工具调用行、`--verbose` 加 thinking、`--raw` 出 JSONL、`--line <n> [--context <k>]` 取窗口（行号来自 `session search`）。
- `yomi session search <词> [-s <id>]`：跨会话全文检索（含工具参数与结果），输出 `L<行号> [role] 片段`，行号直接喂 `cat --line`。
- `yomi session send <消息> [-s <id>]` 的时机语义：不加 flag = **执行完才收到**（排队成新消息，起新任务用）；`--steer` = **执行中即收到**（注入当前 run）——纠偏用 steer，不打断不起新回合。
- `yomi session wait [-s <id>]`：轮询至静默（`phase=idle`、无 running 子 agent、无后台 shell）后 exit 0；首个探测失败（会话不存在/daemon 不在）exit 2；`--timeout <秒>` 超时 exit 3（默认永不超时）。`send` + `wait` = 驱动兄弟会话的最小回路。
- pending 队列：`session mailbox` 查看、`mailbox-remove <mbx_>` 撤回、`mailbox-clear [--steer|--queue]` 清空——只动 pending、不杀 run。
- `yomi session cancel [-s <id>]`：停 agent loop，会话保留。
- 新话题起新会话：`channel new-thread --chat <oc_> --text <任务>`——返回 session_id/thread_url，可接 `send --steer` / `session wait`。
- 群观察模式：`rpc set_channel_watch '{"chat_id":"oc_…","on":true}'`——该群全部消息进该群会话本人（返回其 session_id）。
- 新建空会话：`rpc create_session '{}'` 返回新 session_id（可选 `working_dir`/`model_key`/`auto_approve_level`）。
- 运行态（走 `yomi rpc`）：`get_session '{"session_id":"sess_…"}'` 看 `phase`；`list_running_sessions` 看在跑会话（后台 shell 嵌在 `background_shells` 字段）；`list_subagents '{"parent_session_id":"sess_…"}'` 看直接子 agent。
- checkpoint：`rpc get_checkpoints` 列表；回滚在 TUI `/rewind`。
- 规则文件两层（spawn 时原文注入 system prompt，只在用户要求时更改）：channel rules `<data_dir>/channels/rules/<chat_id>.md`（全群会话）、session rules `<data_dir>/sessions/rules/<session_id>.md`（当前 session）。IM `/rules` 查看生效内容。

注：pending 队列可经 `session mailbox` 查看，但 `session wait` 的静默判定不含它——quiescent 时仍可能有已排队未消费的消息。
