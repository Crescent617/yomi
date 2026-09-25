# cron

定时任务：到点给会话发消息（agent 响应）或执行 shell 命令。

- `yomi cron list|get|create|update|pause|resume|delete`；`yomi cron trigger <id>` 立即手动触发一次（调试用）。
- schedule 为 5–7 字段 cron 表达式（6 字段带秒、7 字段带年），按本地时区解释。
- 一次性任务：`--max-runs 1` + 近未来 schedule。
- shell 类 job 退出码 **42** = 自我完成：标记 `Completed` 不再调度（仅调度执行兑现，手动 `trigger` 不生效）。
- `--precheck <命令>`：sensor 门——每次调度触发前先跑，exit 0 才执行（message 类 job 其 stdout 附加进消息）；非零静默跳过，不计 run 数、不记错误。
- message 类 job 不给 `--session` 时每次运行开新独立会话：`--work-dir` 指定新会话的工作目录（不给时——会话内 cron 工具创建继承当前会话 cwd，CLI 创建落默认 workspace）；绑 `--session` 时 `--work-dir` 无意义（显式报错）。shell 类的 `--work-dir` 是命令的 cwd。
- `--expires-at <RFC3339>` 设过期时间；`yomi cron update <id> --max-runs 0` / `--expires-at never` 可清除限制。
