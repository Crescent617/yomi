# Memory Index

<!-- 一行一事实：- [主题](topics/xxx.md) — 一句话摘要。细节写进 topics/，索引保持精炼（≤200 行）。-->

- [发版 release-it](topics/release-it.md) — 并行发版撞车时 rebase --skip 丢弃本地 release commit、删本地同名 tag 拉远端、再 bump 下一版本。
- yomi E2E 隔离测试 daemon：见 `.agents/skills/yomi-e2e/SKILL.md`（`YOMI_CONFIG` + `YOMI_SOCKET` 两环境变量隔离，已实测验证）。
- schemars 1.x：chrono feature 叫 `chrono04`（不是 `chrono`）；带 doc comment 的枚举 variant 会拆成 oneOf/const 分支而非扁平 enum（doc 变 description）。wire/mod.rs 的 `ReqMethod` 已 derive JsonSchema，是 `yomi rpc --help` 的单一事实源；复杂字段用 `#[schemars(with = ...)]` 打住防层叠。
- 已知空洞：`ShutdownSession` 是空操作——dispatcher 直接返回 Ok(null)（da364ca3 重构时打桩），CLI `session stop` 实际调 cancel 且忽略错误（文案也错印 cancelled），GUI shutdownSession 同样落空。待决：补真实现（从内存移除 session）或删命令/方法。
- kernel 测试凡是碰进程 env / `INJECTED_ENV` 的必须用 `config_test.rs` 里的 `ENV_TEST_LOCK` 串行化——`clear_injected_env` 会抽干整个全局 map，并行 test 线程下互相 race（实测偶发失败）。
- `[env]` 配置语义是**覆盖** host 同名变量且不可逆（host 原值不备份）；`INJECTED_ENV` 跟踪仅为 daemon 子进程 env_remove / GUI 重启清理已删条目，勿当死代码删。
