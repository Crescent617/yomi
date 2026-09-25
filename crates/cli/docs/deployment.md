# deployment 契约

## readiness 探针

K8s readiness 探针：`exec: ["yomi", "rpc", "hello"]`——与 daemon
完成 wire 握手即 exit 0（就绪）；连不上或握手失败 exit 非 0。
建议配 `timeoutSeconds: 3` 兜底连接挂起。

语义：

- 探的是真 round-trip：进程死、启动未完成（listener 未 bind）、
  握手失败（协议版本不匹配、daemon 卡死）均报未就绪。
- 无状态文件，异常退出（SIGKILL/OOM/断电）无残留问题。
- 正常关停期间握手仍成功（intake 闸不拦握手）——Pod 摘流量靠
  K8s Terminating（SIGTERM 即从 endpoints 移除），不依赖
  readiness 失败。
