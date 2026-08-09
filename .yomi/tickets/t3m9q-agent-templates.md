---
title: Agent 模板机制（P1）
status: done
owner_session_id: sess_01KZJE2RJ6QQNX64ZSDWN6TFJC
created_at: 2026-08-09T10:00:00+08:00
---

实现 subagent SP 模板：`agent` 工具加 `template` 参数，conductor resolve `ROLE.md`。

## 验收标准
- 模板文件在 spawn 时被解析并生效（base_prompt 替换）
- 简化收敛后无残留（frontmatter/tools_block/skill 全部移除干净）

## Result

已完成（2026-08-09，经两轮评审 + 简化收敛后的终态）。`agent_tmpl` 模块三层合并（内置 `include_str!` / 全局 `~/.yomi/agents/` / workspace `.yomi/agents/`，同名覆盖）；ROLE.md 纯 markdown 无 frontmatter；发现由 `agent` 工具 desc 自足（内置一句话用途 + 每目录 INDEX.md 索引约定）；错误列可用列表兜底。conductor spawn 时换 base_prompt（空 body/文件丢失均回落默认 + warn）；model/skills/工具集全继承。sessions.template 列（v20）落库供观测（v21 tools_block 列休眠，机制已拆、git 可溯）。内置 planner/verifier/explorer（对齐 CC，无 builder）。`NewSession` 参数结构体重构消灭位置参数 None 汤。无单独 skill（agent-templates 已从 yomi-extensions 删除）。
