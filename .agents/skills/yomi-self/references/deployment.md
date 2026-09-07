# deployment 契约

容器/K8s 部署与健康探针（health 判断逻辑）。daemon 本体无 HTTP 端口，
健康信号走文件系统。

## readiness 标记

`<data_dir>/state/intake`：**存在 = intake 开放**（可接活）。探针
`test -f <该文件>` 即可。生命周期（属主 = KernelServer）：

| 时机 | 动作 | 位置 |
|---|---|---|
| boot | 先删（清 crash 残留，PVC 跨重启共享，必须最早） | `server.start()` 开头 |
| intake 全开（通道+cron+RPC 起完） | 创建 | `server.start()` 末尾 |
| 关停第一步（关 intake） | 删除 | `KernelServer::shutdown()` 开头 |

不带 server 的本地 kernel（`yomi run`/`tui`）不创建也不删除——共享
`data_dir` 时不会摘掉在役 daemon 的 readiness。

## 关停时序（outside-in，探针语义挂钩点）

```
daemon_down hooks（socket 仍在服务，脚本可回连 CLI；每条 30s 顶）
① intake.cancel()      —— 删标记；conductor spawn 闸、通道接收三环、
                          cron 触发与取新任务、RPC 起 run 同时关闭
② 风停                  —— 每会话一次 Shutdown，等落地 ≤60s + 1.5s settle grace
③ kernel.shutdown      —— conductor 停、persist drain ≤10s
④ server.shutdown      —— 通道投递链、连接、wire forwarder 最后死
```

CLI `daemon restart`、Restart RPC（GUI restart kernel）、SIGTERM 全部
走同一序列——标记经历一次"删→重建"，探针看到一个 not-ready → ready
周期。

## K8s 要点

- **副本与策略**：`replicas: 1`；`Deployment` 必须显式
  `strategy: {type: Recreate}`（默认 RollingUpdate 的 maxSurge 会新
  旧并存——双连同一飞书 app 重复处理消息），或用 `StatefulSet`
  （默认有序逐个替换，天然无并存）。RWO 卷跨节点挂载失败是意外护栏。
- **探针**：readiness `exec: ["test", "-f", "/data/state/intake"]`
  （periodSeconds 5）。readiness 失败只摘流量不杀 pod。liveness 暂
  不上（要上就 exec `yomi daemon status` 且阈值保守）。
- **宽限**：`terminationGracePeriodSeconds ≥ 120`（罩住 down hooks
  + 60s 风停 + 1.5s + 10s persist + 5s 连接排空）；SIGKILL 提前截断
  会让排空作废（孤儿卡复活）。
- **PID 1**：容器里 yomi 应为 PID 1（或用 tini），否则 SIGTERM 到不
  了信号监听器。推论：daemon 是 PID 1 时 wire restart 的替代进程活
  不下来（PID namespace 随老进程退出拆除），实际由 kubelet 拉起——
  效果等价，但 env 以 pod spec 为准，别指望 wire 重启的 env 继承。
- **存储**：`$YOMI_DATA_DIR` 挂 RWO 卷（sqlite/session/资产续命）；
  stale socket/pid 与残留标记 boot 时自愈。
