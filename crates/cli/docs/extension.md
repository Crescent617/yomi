# extension 契约（扩展包）

扩展包 = 一个目录 + 一个 `ext.toml`，把多种扩展资源（cron/hooks/bin/
snippets）作为**一个东西**安装与卸载。设计全文见
`docs/design/ext-packages.md`。运行时语义不变：包只改变资源的到达与
撤离方式，落地后各资源受各自契约管辖（执行位开关、cron ensure、
snippet 快照拼装）。

安装源默认是 GitHub（同 nvim 插件 / npx skills 的玩法）：
`owner/repo[/子目录][@ref]` 或 `https://github.com/...`；本地目录只
用于开发。install 一律实体复制进 `extensions/<名>`——源仓之后怎么
变都不影响已装内容，重装才是更新。

**快速体验**（仓库自带示例包）：

```bash
yomi extension install ~/repos/yomi/examples/extensions/demo
yomi extension list
yomi extension remove demo
```

装完可以：任何 yomi 子进程里跑 `demo-hello`、对 agent 说 "demo ping"
（snippet 约定生效）、`yomi cron list` 找到 `ext:demo:hello` 后
`yomi cron trigger <id>` 手动触发一次。

## 包格式

```
<包根>/
├── ext.toml                   # 唯一 manifest
├── snippets/*.md              # 可选：SP 片段（约定式，文件名排序）
├── prompts/*.txt              # 可选：cron message_file 引用的素材
├── bin/<可执行文件>            # 可选：挂进 <data_dir>/bin（已在 PATH 上）
└── hooks/<point>/<entry>      # 可选：hook 条目（裸文件或 <名>/run 目录）
```

### ext.toml

```toml
[ext]
name = "memory-system"        # 必填：字母开头 [a-z0-9-]，≤32
version = "0.1.0"             # 必填
description = "..."           # 必填

[[cron]]
name = "dream"                # 条目名 ≤48；cron 全名 = ext:<扩展名>:<条目名>
schedule = "0 4 * * *"        # 5/6 段，本地时区
message_file = "prompts/dream.txt"   # 与 message 二选一
# message = "内联文本"
# work_dir = "..."            # 可选：per-run 会话工作目录
# precheck = "..."            # 可选：传感器闸门命令
```

manifest 内相对路径不得越出包根（`../` 逃逸在 install 时拒绝）。

## 命令

```bash
yomi extension install <来源>   # GitHub 简写/URL（默认玩法）或本地目录
yomi extension list            # name/version/health/desc/source/资源计数
yomi extension remove <名>     # cron 前缀清扫 + 挂载指向判定回滚
```

全部经 daemon（需 daemon 在跑），与 `yomi cron` 同轨道。

## install 语义（取货 + 复制，重装即更新）

`name` 与 install 同规则校验（字母开头 `[a-z0-9-]` ≤32）；remove 入口
同样校验，`../` 穿越硬拒。

1. 取货：GitHub 源 `git clone --depth 1`（`@ref` 为 40 位 sha 时全量
   clone + checkout）到临时目录，支持 `owner/repo[/子目录][@ref]`；
   本地目录直接用。clone 超时 3 分钟，clone 落盘超 512MB / 5 万文件
   即拒，网络失败不脏任何槽位。
2. 复制进 `extensions/<名>`：一律先复制到隐藏临时目录 `.<名>.tmp`
   再 swap 就位（fresh 与刷新同路径——kill -9 落在复制中途只留
   `.tmp` 孤儿，下次 install 清掉，不留半个无记录的槽位）。逐文件
   ≤1MB、总量 ≤256MB、≤1 万文件，超限即拒；槽位已有且注册表证明
   是上次本包安装的 → **原位刷新**；其他（用户目录、无归属证明的
   残留）→ **拒绝**，绝不覆盖。
3. 清扫旧挂载：凡 symlink 精确指向本包、而新包没有声明的 hook/bin
   挂载一并摘除（**扫 `hooks/`、`bin/` 挂载树本身**，不信旧记录
   名单——上一次刷新中断留下的挂载可能既不在旧名单也不在新包
   里）。悬空 symlink 不挡道、不参与 health，留着只会害后面装同名
   槽位的扩展。
4. 挂载 hooks/bin：逐槽位——空则建 symlink；已指向本包则跳过；
   被用户或其他扩展占用则**整体拒绝并列出占用**（已建部分不回滚，
   重跑 install 收敛）。
5. 收养 cron：ensure-by-name（缺才建、已存在不动，防覆盖手改；
   更新内容请用 `yomi cron update`，清扫只在 remove）。消息文本从
   已装目录读取。
6. 写注册表 `extensions/ext.lock`（单文件，Cargo.lock 式
   `[[extensions]]` 条目）：来源（source/rev）、内容 hash
   （blake3）、资源清单、安装时间，原子整表重写。`ext.toml` 是作者
   的 manifest，**原封不动**。这是唯一注册表：没有 sqlite 表，目录
   + 一个 lock 即全部状态。
   - 复制一完成就先落一条 provisional 条目（资源沿用旧记录，fresh
     安装为空表），挂载/cron 任一步失败都在这之后——重跑 install
     据此原位收敛；remove 精确回滚 = 条目旧资源 ∪ 包目录扫描 ∪
     cron 前缀清扫，不会卡在"occupied 但删不掉"。
   - lock 在包目录外：包内容完全来自作者，作者随包自带 ext.lock
     伪造不了所有权——包内那个只是普通文件，照复制、不解析。

bin 内的可执行文件装完即在 PATH 上（`<data_dir>/bin` 由内核注入所有
子进程）：snippet/文档只写命令名，不写路径。

注意：cron 消息文本在 install 时读进 cron 表——重装前 hooks/bin/
snippets 改动源仓不影响已装内容，**cron 消息也不更新**（ensure 不
覆盖原则）。改 cron 用 `yomi cron update`，或 remove + install 全量
刷新。

## remove 语义

cron 按 `ext:<名>:` 前缀清扫 → 摘 hooks/bin 挂载（**仅当** symlink
文本目标仍指向本包；被换掉的留下并 warn）→ 删 `extensions/<名>`
（仅当注册表有条目证明是本包装的内容；用户目录留下并 warn）→ 注册
表条目同步移除。卸载不需要注册表在场也能精确回滚 cron 与挂载
（前缀 + 指向判定），注册表只是让目录删除有归属证明。

## health（`extension list` / `doctor`）

| 值 | 含义 |
|---|---|
| `ok` | 注册表有条目且内容 hash 与当前目录一致 |
| `modified` | 注册表有条目但 hash 对不上（本地手改过包内容） |
| `foreign` | 目录在但注册表无条目——用户手放，或 ext.toml 损坏/解析失败（占位可见，内容不可信） |
| `unreadable` | hash 重算时包文件读不了（权限/损坏） |
| `oversized` | 包内出现超 1MB 的文件，或总量超 256MB / 文件数超 1 万（hash 拒绝计算；多半是误放大文件进包目录） |

## snippet 拼装

`extensions/*/snippets/*.md` 在会话 spawn 时按（扩展名字典序 →
文件名序）拼进 system prompt，位于项目 memory 之后、skills 索引
之前，标题为 `# Extension: <名>`。无 snippet 的扩展零 prompt 成本；
源破损的扩展跳过（health 列暴露）。单文件 16KB 上限，超出截断带
标记。

## 边界

- v1 不支持 tools/skill 资源挂载（二期）；bin 只收扁平文件，多文件
  工具走外挂 tools/（`yomi doc tools`）。
- snippet 单文件 16KB 上限，超出截断带标记；同一扩展多个 snippet 各
  自带 `# Extension:` 标题。
- 单文件 1MB / 总量 256MB / 1 万文件：包是"约定 + 小脚本"的载体，
  藏超大文件按恶意/损坏处理，install 直接拒绝。
- 子目录源（`owner/repo/ext/foo`）：整仓 clone 后只复制子目录；
  仓里多个扩展包互不干扰。
- 扩展的 hook 与用户 hook 按条目名字典序混排：扩展条目约定 `50-`
  起的名字自定位。
- 信任边界 = 能写 `<data_dir>` 的主体 + 源仓写者：包内脚本以 daemon
  子进程全权执行，bin 命令可遮蔽系统同名命令（PATH prepend）；装
  GitHub 源等于信任该仓的写者（其后续 commit 会在你重装时进入）。

## 排障

| 现象 | 原因与处置 |
|---|---|
| `mount conflict: ...` | 槽位被用户文件或其他扩展占用。挪走冲突项后重跑 install（幂等）。 |
| install 报槽位 occupied | `extensions/<名>` 有目录但注册表无条目（用户手放或上次装失败残留）。确认无用后手动删目录，或换个包名。 |
| `extension list` 显示 `modified` | 装完手改过包内容。属预期则忽略；想回到安装态 remove + install。 |
| `extension list` 显示 `foreign` | 注册表无条目：用户手放、ext.toml 损坏，或上次装失败残留。确认无用后手动删目录，或换包名。 |
| `extension list` 显示 `unreadable` | 包文件权限/损坏致 hash 读不了。修权限或 remove + install 重建。 |
| `extension list` 显示 `oversized` | 包目录混入了超 1MB 的文件（或总量/数量超限）。移走它，或 remove + install。 |
| install 成功但 cron 报 `exists, untouched` | ensure 语义：同名 job 已存在，未覆盖。要改内容用 `yomi cron update`，或 remove + install。 |
| `invalid package: ...` | ext.toml 校验失败（名字规则、message 二选一、`../` 逃逸、schedule 无未来触发点、同名条目重复）。按报错逐条修。 |
| 想看 snippet 拼装结果 | `yomi rpc preview_system_prompt`（可传 working_dir）返回将拼进新会话的完整 SP，grep `Extension: <名>` 即见本扩展的段。 |
| snippet 改了没生效 | 扫描有 60s 缓存（同 skills）：等约一分钟、或新开会话 spawn 时生效。 |
| bin 命令 not found | 确认文件有执行位（install 时无执行位会 warn 但仍挂载）；已开会话的下一条命令即可解析——PATH 每次 spawn 子进程时注入。 |
| remove 报槽位留下 | 槽位内容被换过（指向判定保护用户数据），手动检查后再决定。 |
| `git clone failed` | 源仓不存在/无权限/ref 名错。核对 `owner/repo[/子目录][@ref]` 拼写。 |

## 写一个扩展包

最小包只需三样：`ext.toml` + `snippets/<名>.md` + 想要的资源目录。
参考 `examples/extensions/demo`（cron + bin + snippet + 通知型 hook
各一）。写完本地 `yomi extension install <目录>` 验证，再考虑进
yomi-extensions 这类资产仓。
