---
title: Agent 模板机制（P1）
status: done
owner_session_id: sess_01KZJE2RJ6QQNX64ZSDWN6TFJC
created_at: 2026-08-09T10:00:00+08:00
---

实现 subagent SP 模板：`agent` 工具加 `template` 参数，conductor resolve `ROLE.md`（tools_block 交集 + model_key 覆盖）。

## 验收标准
- 模板文件在 spawn 时被解析并生效（base_prompt 替换 + 工具交集）
- tools_block 只能收窄不能扩大

## Result

已完成（2026-08-09）。`agent_tmpl` 模块三层合并（内置 include_str! / 全局 / workspace 同名覆盖）；`agent` 工具 `template` 参数实时 resolve、未知名报错附可用列表；conductor spawn 时换 base_prompt + tools_block 收窄 + skills 白名单 + model_key 覆盖；sessions.template 列（migration v20）落库供观测。顺手重构：`SessionStore::create` 改 `NewSession` 参数结构体（消灭 6 个位置参数的 None 汤）。内置 planner/builder/reviewer；`agent-templates` skill 已发布到 yomi-extensions 并本机生效。测试：kernel 1105 全过（含 3 个模板路径新测试）。
