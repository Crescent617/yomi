# 调试

- `yomi run "<prompt>"`：headless 一次性运行。退出码：0 成功 / 2 失败 / 3 超迭代 / 124 超时 / 130 取消。脚本里要执行工具须 `--yolo` 或 `--auto-approve`。
- shell 工具与 cron shell 任务的子进程都注入 `YOMI_DATA_DIR`（有会话时加 `YOMI_SESSION_ID`）——脚本可据此回连 CLI（如 `yomi session cat "$YOMI_SESSION_ID"`）。
- `yomi events [-s <sid>]`：事件 NDJSON 流；`--all` 跨会话仅实时（无回放）；`--after-event-id` 断点续传。
- `yomi rpc <method> [params-json]`：wire 协议逃生舱口；`--help` 列全部方法、`<method> --help` 显示参数 schema（无需 daemon）。流式方法（subscribe）只回 ack，事件流用 `events`。
- `yomi usage`：token 用量统计（`-n` 天数，`--model`/`--provider` 过滤）。
- `yomi gc`：清理过期会话、无属主文件、cache.db。默认 dry-run，`--yes` 才真删；`--vacuum` 压缩。`[gc] auto` 配置可每天自动清。
- `yomi doctor`：daemon/channels/cron/存储/配置整体自检，任一 ❌ 即 exit 1（重启后自检、发版门禁）。
- 日志：`~/.yomi/logs/daemon.<date>.log`（`tui.`/`run.` 前缀同理）。
- 与生产并行的隔离测试 daemon（随便折腾）：

  ```sh
  export YOMI_DATA_DIR=/tmp/yomi-test YOMI_SOCKET=/tmp/yomi-test.sock YOMI_CONFIG=/tmp/yomi-test/config.toml
  cp ~/.yomi/config.toml "$YOMI_CONFIG"   # 并删掉其中的 [[channels]] 段
  nohup yomi daemon start >/dev/null 2>&1 &
  yomi doctor
  ```

  三个坑：config 不随 YOMI_DATA_DIR 走——不覆盖 `YOMI_CONFIG` 时 channels 会双开，和生产 daemon 抢同一份消息；`daemon start` 是前台内部命令，必须后台化；要测 agent 会话内的新 CLI，给 daemon 的 PATH 前置构建目录。要零 skill 环境再加 `HOME=<隔离目录>`（全局层 `~/.agents/skills` 跟 HOME 走）。或 `yomi run --fg` / `yomi tui --fg` 用进程内核，完全不动 daemon。
