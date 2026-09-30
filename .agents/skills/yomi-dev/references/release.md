# 发版落地

前置纪律（hrli 立的规矩）：

- 发版须 hrli 明确授权；改完代码他实物验证点头前不 push（2026-08-19）。
- 只要 fix 就要 review；amend/commit 之前必须完成当轮评审，无例外（2026-08-21/22）。
- 打 tag 前在 HEAD 本地复跑 `just ci` 全绿——CI 门禁有洞（release.yml 无 fmt gate），"tag CI 绿"≠"HEAD 干净"（v0.10.20 带病发出，2026-09-05）。
- 发版前把功能改动与 release commit 分开提交——并行发版撞车时便于丢弃本地 release commit（见步骤 5）。

## 1. 工作区与版本

```bash
git diff --quiet && git diff --staged --quiet   # 不干净先 stash/提交
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
git rev-parse "v${VERSION}" >/dev/null 2>&1 && echo "tag v${VERSION} 已存在，需 bump" || echo "可直接发版"
```

bump 一律用 `scripts/bump-version.py`（`--level patch|minor|major` 或 `--set X.Y.Z`）——7 个文件单点管辖（Cargo+lock、tauri.conf、双 package.json 等），绝不手工改（手工漏 tauri.conf.json+gui npm 两处，v0.10.13 首发失败，2026-09-02）。

## 2. CHANGELOG

0. **轮转**：把 `[Unreleased]` 改为 `[<新版本>] - <日期>`，其上重开空 `[Unreleased]`——bump 脚本不做这步；漏了 release.yml 抽不到本版本 section，GitHub Release body 为空（v0.8.2/v0.9.0 各踩过一次）。
1. **核对覆盖度**：`git log v<上个版本>..HEAD --oneline`，新条目覆盖本次发布包含的全部提交；发现上个版本漏写的已发布内容，先补写再写新条目。
2. **写法遵循 CHANGELOG.md 顶部《编写要求》**：面向用户、一条一行一句话、不写内部实现、配置与命令点名。
3. **查结构**：`grep -n '^## \[' CHANGELOG.md`——每版本一个标题、版本号严格倒序。
4. 本 section 即 release note：release.yml 自动抽取为 GitHub Release body，不要手动在 GitHub 上编辑 release 内容。

## 3. tag + push

```bash
RELEASE_VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
git tag -a "v${RELEASE_VERSION}" -m "Release v${RELEASE_VERSION}"
git push origin main
git push origin "v${RELEASE_VERSION}"
```

## 4. 等 CI

push tag 后先 `cd ~/repos/yomi` 再操作 gh（后台 shell 的 cwd 不保证在仓库里，2026-09-01）。拿 run id 后台 watch，不轮询阻塞会话：

```bash
RUN_ID=$(gh run list --limit 1 --json databaseId -q '.[0].databaseId')
gh run watch "$RUN_ID" --exit-status --interval 60   # 后台 shell 跑
```

**终态以一次 `gh run view <id> --json status,conclusion` 为准**——watch 有假死/假完成前科（代理 EOF：v0.10.12 两起、v0.9.21 幻影"全绿"），即使 watch 报成功也要核实。

- 非 0 = CI 失败：`gh run view <id> --log-failed`，报告用户，中止后续步骤。
- 0 = 成功：进入落地核实。

## 5. 并行发版撞车

push main 被拒（fetch first）时不要强推：

1. `git fetch origin` → rebase；本地 release commit 通常已过时，直接 `git rebase --skip` 丢弃（更干净：`git reset --hard origin/main && git cherry-pick <功能commit>`，只重放功能）。
2. 本地同名 tag 未推送过：`git tag -d <v>` 后 `git fetch origin refs/tags/<v>:refs/tags/<v>` 拉回远端 tag。
3. 基于远端最新版本 bump 下一个版本再发。

（2026-08-10/08-11/08-13 三连撞，均据此恢复：如准备 0.7.67 时远端已发 0.7.68，最终发 0.7.69。）

## 6. 落地核实（不信通知层，2026-08-27）

1. `gh release view "v${RELEASE_VERSION}" --json body --jq .body` 与本版本 CHANGELOG section 一致。
2. `brew update && brew info crescent617/tap/yomi | head -2` 显示新版本号。tap 由 CI 自动推送，**永不手动改 tap**——手动 commit rebase 会撞 "patch contents already upstream"（v0.9.x）。

## 7. 本机升级 + 落地（dogfood）

前置：CI 已绿、tap 已 bump。

1. **CLI**：`brew update && brew upgrade yomi`（不动 yomi-app，两个分开升）。验 `ls -la /opt/homebrew/bin/yomi` 必须是 symlink——不是就 rm + `brew link --overwrite`（普通文件遮蔽 brew link，报成功但 `--version` 仍旧版，v0.9.6，2026-08-22）；`yomi --version` == 目标版本。
2. **GUI 本体**：`brew info --cask yomi-app | head -1` 显示的版本 == 本次发布版本才走步骤 3 的退升开；没新版则跳过，常态重启即可。
3. **重启**：
   - **常态：自杀式 daemon restart**——`nohup sh -c 'sleep 8; yomi daemon restart' >/dev/null 2>&1 &` 立即结束，**绝不在同一条命令里 sleep+验证**（进程已死，必误报诱导重试）。执行重启的 shell 要富 PATH：必须含 `/etc/profiles/per-user/hrli/bin`（rg/cargo 在 nix profile；半富 PATH 曾让生产 grep 工具残废 20 分钟，2026-08-25）。
   - **仅 GUI 本体升级时退升开**（退出 GUI → 升 cask → 重开；cask 磁盘升级不替换运行中进程，daemon restart 代替不了；杀 GUI 同时杀掉内嵌 daemon 与本会话——脚本必须脱离 agent 进程存活，否则随 daemon 一起死、后半段不执行；逐字用）：
     ```bash
     nohup sh -c 'sleep 5; osascript -e "quit app \"Yomi\"" 2>/dev/null; sleep 4; \
       pgrep -f yomi-gui >/dev/null && kill $(pgrep -f yomi-gui) 2>/dev/null; sleep 2; \
       brew upgrade --cask yomi-app >>/tmp/yomi-cask-upgrade.log 2>&1; sleep 3; open -a Yomi' >/dev/null 2>&1 &
     ```
4. **版本核实**：`yomi rpc hello | grep version`、`yomi --version` 全 = 目标版本；走了退升开再加 `plutil -p /Applications/Yomi.app/Contents/Info.plist | grep ShortVersion`。`rpc hello` 报旧版 = cask 没升成（内嵌 daemon 版本即 app 本体版本），回步骤 3 退升开。
5. **自检**：任何杀 GUI/daemon 的操作（退升开、探针、测试、清理）执行前，都先排好下面这个一次性 cron 再动手。先 `yomi rpc list_running_sessions` 确认除本会话无在跑，`yomi doctor` 全过。预排（sqlite 持久化跨重启；**name 必须带版本号**——cron create 同名幂等，返回旧任务不更新，2026-08-19；排 cron 先 `date '+%H:%M %z'` 对表本机时区——按本机时区解释，UTC 心算易排进"已过时刻"、next_run 跳次年，v0.10.11，2026-09-02）：
   `yomi cron create --name restart-self-check-<版本> --session <本会话id> --max-runs 1 --schedule "$(date -v+2M '+%-M %-H %-d %-m *')" --message '自检重启：yomi doctor + yomi --version，简报结果'`
   醒后核对：`yomi --version` == 本次版本、日志无 ERROR、cron active/failed、channel receiving。

约束：

- macOS 上 daemon 保持 GUI 内嵌形态：别从 SSH/终端另拉 daemon（TCC 责任主体 sshd-keygen-wrapper，/Volumes/Data 被静默拒且日志无痕；双 daemon 抢 socket）。watchdog 与 system LaunchDaemon 已退役（登录卡死前科）。
- 重启代价：飞书 ws 僵尸连接约 10 分钟，期间事件会丢——挑用户空闲时做。
