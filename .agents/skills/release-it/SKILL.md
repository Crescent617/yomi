---
name: release-it
description: "Release a new version of yomi and roll it out to the local dogfood machine. Use when asked to 发版/release yomi（含发版后等 CI、tap 检查、本机升级重启）。"
---

# Skill: release-it

发布 yomi 新版本。核心流程：检查当前版本是否已有 tag → 若无则直接发版，若有则先 bump 版本 → 打 tag → push。

## 步骤

### 1. 确认工作区干净

```bash
git diff --quiet && git diff --staged --quiet
```

若不干净，先 `git stash` 或提交当前改动，确保发版 commit 独立清晰。

### 2. 读取当前版本

```bash
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
echo "Current version: $VERSION"
```

### 3. 检查 tag 是否存在

```bash
if git rev-parse "v${VERSION}" >/dev/null 2>&1; then
    echo "Tag v${VERSION} already exists. Need to bump."
    NEED_BUMP=1
else
    echo "Tag v${VERSION} does not exist. Ready to release."
    NEED_BUMP=0
fi
```

### 4. 若需要 bump 版本

使用 `scripts/bump-version.py`。默认 bump `patch`，除非用户特别说明

```bash
# 默认 patch
python scripts/bump-version.py --level patch

# 或 minor / major
python scripts/bump-version.py --level minor
```

脚本会自动同步更新以下文件：
- `Cargo.toml`
- `crates/gui/tauri.conf.json`
- `crates/gui/package.json`
- `crates/gui/package-lock.json`
- `crates/gui/frontend/package.json`
- `crates/gui/frontend/package-lock.json`

### 5. CHANGELOG

0. **轮转**：把 `[Unreleased]` 标题改为 `[<新版本>] - <日期>`，并在其上重开空的 `[Unreleased]`——bump 脚本不做这步；漏了的话 release.yml 抽不到本版本 section，GitHub Release 的 body 会是空的（v0.8.2/v0.9.0 各踩过一次）。
1. **先核对覆盖度**：`git log v<上个版本>..HEAD --oneline`，确保新条目覆盖本次发布包含的**全部**提交；若发现上个版本的条目漏写了已发布的内容，先补写再写新条目。
2. **写法遵循 `CHANGELOG.md` 顶部的《编写要求》**：面向用户、一条一行一句话、不写内部实现、配置与命令点名。不要写成长段从句。
3. **写完检查结构**：`grep -n '^## \[' CHANGELOG.md`，确认每个版本只有一个标题、版本号严格倒序、没有内容串到别的版本下。
4. **这段就是 release note**：release.yml 会自动把本版本 section 抽取为 GitHub Release 的 release note，无需也不应手动在 GitHub 上编辑 release 内容。

### 6. 打 tag

用最终版本（bump 后的或原本的）打 tag：

```bash
RELEASE_VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
git tag -a "v${RELEASE_VERSION}" -m "Release v${RELEASE_VERSION}"
```

### 7. Push

```bash
git push origin main
git push origin "v${RELEASE_VERSION}"
```

### 8. 等 CI（异步，不轮询）

push tag 后拿 run id：

```bash
gh run list --limit 1 --json databaseId -q '.[0].databaseId'
```

用后台 shell 等，完成会自动通知，会话不阻塞：

```bash
gh run watch <run_id> --exit-status --interval 60
```

不排定点 cron 猜 CI 时长、不同步 watch 阻塞会话。完成通知到达后：

- exit code 非 0 = CI 失败：`gh run view <run_id> --log-failed` 查日志，报告用户，中止后续步骤。
- exit code 0 才进入下一步。

### 9. 发版落地检查

CI 成功 ≠ tap 已更新（CI 还要推 formula/cask）。先排一次性 cron（max_runs 1）或 `nohup sh -c 'sleep 90; ...' &` 延迟 1-2 分钟再查，不立刻查。

1. **release note**：`gh release view "v${RELEASE_VERSION}" --json body --jq .body`，内容与本版本 CHANGELOG section 一致。
2. **tap 已更新**：`brew update && brew info crescent617/tap/yomi | head -2` 显示新版本号。tap 由 CI 自动推送，永远不要手动改 tap——手动 commit rebase 会撞 "patch contents already upstream"（v0.9.x 踩过）。

### 10. 本机升级 + 重启（dogfood 机）

顺序固定：升级 → 验软链 → 简报 → 排自检 cron → 延迟重启。

1. 升级 CLI + GUI：`brew update && brew install -y crescent617/tap/{yomi,yomi-app}`
2. 验软链：`ls -la /opt/homebrew/bin/yomi` 必须是指向 Cellar 的 symlink（部署脚本曾放普通文件遮蔽 brew link）。
3. 先向用户简报升级内容，再延迟重启：`nohup sh -c 'sleep 8; yomi daemon restart' &`。执行重启的 shell 要富 PATH（瘦 PATH 缺 rg，daemon 起来后 grep 工具残废）。代价：飞书 ws 服务端僵尸连接约 10 分钟，期间事件会丢——挑用户空闲时做。
4. 重启前预排一次性自检 cron（max_runs 1、send_message，sqlite 持久化跨重启）：醒后核对 `yomi daemon status`、`yomi --version` == 本次发布版本、日志无 ERROR。
5. GUI（Yomi.app）磁盘升级不替换运行中进程：提醒用户手动退出重开，daemon restart 代替不了。
