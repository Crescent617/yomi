---
name: yomi-dev
description: "维护 yomi 自身：发版落地与隔离真机 E2E。Use when hrli 授权发版、bump 版本、打 tag、等 CI 终态、升级本机 brew、发版后自检，或飞书通道、daemon、CLI、cron 改动需要真机端到端验证。"
---

# yomi-dev

维护 yomi 仓库本身。两个独立分支，按任务读对应 reference，动手前读完：

- **发版落地**（授权发版、bump、tag、等 CI、brew 升级落地、自检）→ `references/release.md`
- **隔离真机 E2E**（飞书通道、daemon、CLI、cron 改动需真机端到端验证）→ `references/e2e.md`
