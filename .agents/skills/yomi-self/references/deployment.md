# deployment 契约

## readiness 标记

`<data_dir>/state/intake`：**存在 = intake 开放**（可接活）。K8s
readiness 探针 `exec: ["test", "-f", "<该文件>"]` 即可。

生命周期：daemon 完整启动（通道+cron+RPC 起完）后创建；开始关停时
率先删除；boot 先清残留。不带 server 的本地进程（`yomi run`/`tui`）
不创建也不删除，共享 `data_dir` 不会误摘。
