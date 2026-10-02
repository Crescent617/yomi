# Code Mode（程序式工具编排）评估与候选设计

## 背景

2026-09-30 调研了 pi 的 codemode 与 DSH 的 PTC，并评估 yomi 是否需要、以及
如何实现同一能力。结论先行：**概念已想透，实施暂缓**——见文末触发信号。

调研对象（一手资料）：

- **pi codemode**（earendil-works/pi）：模型提交一段 JavaScript，在 **QuickJS
  沙箱**执行，脚本经 `tools.<name>(args)` 调其他工具；只有脚本输出回模型；
  配套 exposure 五级（direct/model-only/codemode/deferred/hidden）+
  BM25 `search_tools()` 解决 MCP 工具数量膨胀；`store()/load()` 跨脚本
  持久化；`// @options` 行自设超时/输出上限。嵌套调用走
  `ctx.executeTool()` 原管线，权限钩子照常。
- **DSH PTC**（DeepSeek Harness preset 之一）：`run_code` 工具，模型提交
  TypeScript 函数体，在 **Node worker thread** 执行（非沙箱，信任等级对齐
  bash），工具以 TS SDK 形态（`tools.bash({...})`）呈现，Promise.all 并发。
  概念源自 Cloudflare 的 Program-Tool-Call 观察：LLM 写程序比填
  tool-call 参数更自然。

两者买的东西相同：**O(n) 的工具往返压成 1 次模型往返 + 中间结果不进
context**。买的是成本与延迟，不买新能力。

## yomi 的候选设计（推演结论）

### 候选一：内嵌 QuickJS（pi 路线）——否决

- 需要新增：JS 引擎依赖、自研 async↔tokio 桥（pi 代码库中最难部分）、
  沙箱中断/超时语义、exposure 工具发现机制。
- 关键认识：pi 需要 QuickJS 的隐含前提是"不想让模型为批量只读操作获得
  bash 权限"。**yomi 的 shell 工具本来就在那里**，这个前提不成立。拿掉
  该约束后此方案剩不下独有收益。

### 候选二：`yomi tool call` CLI 子命令（选定，暂缓实施）

不新增运行时：模型继续用现有 shell 工具，写 bash/python 脚本编排，脚本内
调 `yomi tool call <tool> '<json>' [--format json] [--session <id>]`。

设计要点：

1. **子命令是 daemon 的 RPC client，不自己执行工具**：经 socket 挂到 live
   session，由 session 的 `tool_exec` 管线执行——参数校验、
   `permission::resolver` 定级、checker/conductor 审批全部照常生效。
   嵌套调用逐个过 checker，比 DSH 的"等同 bash 权限"细。
2. **权限语义**：无可挂接 session 时 fail-closed（对齐外挂工具镜像语义），
   不留 `--yolo` 后门。
3. **归属与事件流**：shell 工具 spawn 时注入 `YOMI_SESSION_ID`；
   ToolEvent 加可选 `parent_tool_call_id`；"stream is reality" 不破例，
   conductor 看得见每个嵌套调用。
4. **输出给程序用**：`--format json` 结构化结果（exit_code/truncated/
   full_output_path），脚本 jq/python 过滤；只有最终输出经 shell 截断回模型。
5. **顺带收益**：对人、CI、hook 同样可用。

实现清单（将来上马时）：cli 新 subcommand；daemon dispatcher 加"指定
session 执行工具调用"请求；shell 注入 `YOMI_SESSION_ID`；ToolEvent/wire
加 `parent_tool_call_id`；checker 非交互等待超时→deny；shell 工具描述补
一段编排指引；`evals/harness-e2e.sh` 回归。

主要风险：审批往返依赖用户在线——子命令须带合理超时，超时 deny 并让
脚本报错退出。

### MCP 如何接入（同一轮推演的结论）

kernel 现状：**无 MCP 支持**；能力端口 out-of-proc 形态 = ext/
（`tools/<name>/tool.json` + `run`，静态 manifest、每次调用 spawn）。

- **接法一（kernel 内置 MCP client）**：JSON-RPC、双传输、server 生命周期、
  OAuth、schema 翻译——一整个新子系统。且与 ext 的 spawn-per-call 模型
  本质冲突（MCP server 是有状态长驻进程）。后置。
- **接法二（选定，同样暂缓）**：`yomi mcp add <server>` 安装器：做一次
  `tools/list`，每个 MCP 工具翻译为 ext 条目（manifest 缓存 schema，
  `run` 为通用 shim 回源 server+tool）。kernel 零改动。工具名翻译复用
  ext 已有命名约束；manifest `level` 默认 Caution（MCP 工具能做的事没有
  上限）。
- 触发 promote 到接法一的条件：真实出现有状态 server（浏览器自动化类）
  或 OAuth 需求，或 shim 进程链延迟被实际抱怨。

### 两个设计的协同

MCP-as-ext 工具进 ToolRegistry 后，`yomi tool call` 立刻覆盖 MCP 编排：
pi 需要 exposure 五级 + BM25 撑住的几百个 MCP 工具的批量调用问题，yomi
用 shell + jq 解决。

## 现状覆盖度评估（为什么暂缓）

`yomi tool call` 相对 yomi 现状的增量只剩两条窄缝：

1. 脚本里调 ToolRegistry 的**动态工具**（ext/MCP）——CLI 无对应子命令，
   唯一硬性能力缺口；
2. 对话中编排 permission-gated 的 edit/write 且保持 gate 在环——bash 的
   sed/python 写文件绕过 conductor。但每次嵌套调用仍单独审批，批量化在
   权限侧反而被拆开，使用频率存疑。

其余 code mode 价值的 ~80% 已被现有三件覆盖：shell 工具 + 真 bash/python
（文件循环/过滤/多跳数据流）；yomi CLI 本身会话级可脚本化（`session`/
`cron`/`gc`/`events` 子命令——维护类 O(n) 任务已闭环）；subagent（上下文
隔离的多步任务出口）。

## 决策（2026-09-30，hrli 拍板）

**暂不实施 code mode 与 MCP 接入。** 是优化项不是缺口项；优化项不预测性
地建。设计定型备查，届时为加法。

**同日追加拍板**：shell 工具的 `interpreter` 参数（枚举 default/python/
自定义路径、脚本落临时文件执行）曾实现一版，最终**不做**——现有 skill +
CLI 已覆盖脚本化需求，不值得为解释器选择单独加参数。实现过程留下的唯一
增量认知：脚本喂给解释器的通用约定是「临时文件 + `<解释器> <文件>`」，
优于 `-c`（免平台差异与引号转义），将来若重启此设计可直接沿用。

从"可选"变"必要"的触发信号（满足任一即上马）：

- 真实任务让模型烧掉 10+ 轮机械往返（看 session 记录时心疼 token）；
- ext/MCP 工具数量上两位数，模型需要循环调用它们；
- cron 脚本里反复拼装 CLI 子命令做"类工具调用"的事；
- 出现有状态 MCP server 或 OAuth 需求（MCP 线单独触发）。

届时实施顺序：`yomi tool call` 先行（纯增量、无决策遗留），MCP 安装器
视工具生态需要跟进。
