# Memory Index

<!-- 一行一事实：- [主题](topics/xxx.md) — 一句话摘要。细节写进 topics/，索引保持精炼（≤200 行）。-->

- [launchd 下 daemon 启动卡死](topics/launchd-daemon-hang.md) — launchd bootstrap 的 daemon 必现卡死在 `init_logging` 之前（位置不固定），终端 CLI spawn 秒起；已 bootout 回 CLI 方式，plist 保留。
- [发版 release-it](topics/release-it.md) — 并行发版撞车时 rebase --skip 丢弃本地 release commit、删本地同名 tag 拉远端、再 bump 下一版本。
- [Tauri macOS 菜单吞按键](topics/tauri-macos-menu-keys.md) — 默认菜单 Edit>Undo/Redo 的 key equivalent 在事件进 webview 前被 OS 消费,JS 编辑器 Cmd+Z 全失效;自建菜单摘掉 Undo/Redo 解决。
- yomi E2E 隔离测试 daemon：见 `.agents/skills/yomi-e2e/SKILL.md`（`YOMI_CONFIG` + `YOMI_SOCKET` 两环境变量隔离，已实测验证）。
- schemars 1.x：chrono feature 叫 `chrono04`（不是 `chrono`）；带 doc comment 的枚举 variant 会拆成 oneOf/const 分支而非扁平 enum（doc 变 description）。wire/mod.rs 的 `ReqMethod` 已 derive JsonSchema，是 `yomi rpc --help` 的单一事实源；复杂字段用 `#[schemars(with = ...)]` 打住防层叠。
- 已知空洞（2026-08-14 部分收敛）：wire `ShutdownSession` 仍是空操作桩（dispatcher 返回 Ok(null)，da364ca3 重构时打桩），GUI `shutdownSession` 按钮同样落空；CLI `session stop` 已删除（决定：不要该命令，场景由 rpc 覆盖）。待决：删 wire 方法 + GUI 按钮，或补真实现（从内存移除 session）。
- kernel 测试凡是碰进程 env / `INJECTED_ENV` 的必须用 `config_test.rs` 里的 `ENV_TEST_LOCK` 串行化——`clear_injected_env` 会抽干整个全局 map，并行 test 线程下互相 race（实测偶发失败）。
- `[env]` 配置语义是**覆盖** host 同名变量且不可逆（host 原值不备份）；`INJECTED_ENV` 跟踪仅为 daemon 子进程 env_remove / GUI 重启清理已删条目，勿当死代码删。
