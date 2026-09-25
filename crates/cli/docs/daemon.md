# daemon

- `yomi daemon status` / `restart` / `stop`（`start` 仅供内部调用）。
- `yomi doctor`：daemon、channels、cron、存储、配置整体自检，任一 ❌ 即 exit 1——重启后自检、发版门禁用它。
- 重启路径：CLI `yomi daemon restart`、IM `/restart`（限 `admin_users`）、GUI 改配置自动重启。**进行中的 run 会被打断**——先 `yomi rpc list_running_sessions` 确认没在跑。
- **自杀式重启**（agent 重启自己）：restart 生效时本进程即死——命令必须**立即 exit 0**，不在同一条命令里 `sleep` + 验证（必误报，诱导重试）。先排一次性 cron 自检（重启后照跑），再 `nohup sh -c 'sleep 8; yomi daemon restart' >/dev/null 2>&1 &` 直接结束：

  ```
  yomi cron create --name restart-self-check-<版本号> --session <本会话id> --max-runs 1 \
    --schedule "$(date -v+2M '+%-M %-H %-d %-m *')" \
    --message '自检重启：yomi doctor + yomi --version，简报结果'
  ```

- 日志：`~/.yomi/logs/daemon.<date>.log`（`tui.`/`run.` 前缀同理）。
- `yomi daemon auth-hash [密码]`：算 socket 密码哈希；`--generate` 生成随机 token + 哈希 + 配置指引。鉴权全貌见 `yomi doc config` 的 socket 鉴权节。
- 容器/K8s 部署与 readiness 探针：`yomi doc deployment`。
