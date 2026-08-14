# 发版（release-it）

- 流程见 `.agents/skills/release-it/SKILL.md`；版本号同步靠 `scripts/bump-version.py`。
- **可能有并行 session/机器同时发版**：push main 被拒（fetch first）时不要强推。处理：fetch → rebase（本地 release commit 通常已过时，直接 `git rebase --skip` 丢弃；更干净的做法是 `git reset --hard origin/main && git cherry-pick <功能commit>`，只重放功能、整体丢弃 release commit）→ 本地同名 tag 未推送过，`git tag -d` 后 `git fetch origin refs/tags/<v>:refs/tags/<v>` 拉回远端 tag → 基于远端最新版本 bump 下一版本再发。（实例：2026-08-10 本地准备 0.7.67 时远端已发到 0.7.68，最终发 0.7.69；2026-08-11 本地 0.7.70 与远端撞车、远端已到 0.7.71，最终发 0.7.72；2026-08-13 本地 0.7.74 与远端撞车、cherry-pick 功能 commit 到对方 release 之上后自动合并 config_test.rs/config-schema.json，最终发 0.7.75。）
- 发版前把功能改动与 release commit 分开提交，rebase 时便于丢弃 release commit。
