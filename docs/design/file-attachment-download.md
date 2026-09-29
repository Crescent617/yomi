# 设计文档：飞书文件附件自动下载（file attachment auto-download）

> **状态（2026-09-29）：待拍板**。2026-09-29 事故驱动：用户发 PPT/PDF，
> 会话只收到 `[file: xxx (lark_file_key: file_v3_…)]` 占位文本，agent 不知文件落点，
> 两次误判"没收到"。对比图片（`image_keys` post-gate 自动下载内嵌），
> 文件类消息（`file`/`audio`/`media`）自 v0.10.34 起只注入占位符
> （`feishu_text.rs::attachment_placeholder`），下载完全甩给 agent 猜。
> 本文档定义 kernel 自动下载链路。**拍板后实施。**

## 语义决策

- **< 100MB 自动下载**，占位符就地改写为本地路径：agent 零额外动作
  即可 `read` 文件（pptx/pdf/zip 全走文件路径）。
- **> 100MB 不下载**，保留占位符，追加一条 `<system_reminder>` 提示
  超限并给出手动下载命令（key 仍在占位符里）。
- **100MB = 100 × 1024 × 1024 字节**，等于上限可过，超出即拒。

### 候选方案与取舍

1. **镜像 image_keys 加 struct 字段**（`ChannelMessage.attachments` +
   `HistoryMessage.attachments`，下载签名走 `PlatformAdapter` 新方法）：
   类型上最干净，但两个 pub struct 各加必填字段要同步改 ~30 处测试
   构造点，churn 大、review 面宽。
2. **占位符解析（本文档）**：占位符格式 `attachment_placeholder` 是
   kernel 自己铸造的，语法固定可解析。hub post-gate 扫描**触发消息
   自己的文本块**，按 key 下载、就地改写。零 struct 变更、零测试构造
   点改动，新增面只有：trait 一个默认方法 + feishu 一个实现 + hub
   一个函数 + 三处接线。

选 2。图片不走这条路只是因为图片是二进制内嵌（data URL），没有文
本载体。

## 机制

### 下载（post-gate，与图片同一时机）

`feishu.rs` 实现 `PlatformAdapter::download_message_file`：

```
GET /open-apis/im/v1/messages/{msg_id}/resources/{file_key}?type=file
```

- 与图片同一端点（`download_image` 注释已载明 message key 必须走
  resources 端点），`type=file`。
- `Content-Length` 已知且 > 上限：直接回 `Oversize`，不落地。
- 否则流式写盘，累计超限即删半成品回 `Oversize`（服务端不诚实时
  的兜底）。
- 落盘名取 `file_name` 的文件名片段（去路径成分、去 `..`），冲突
  加 `-1`/`-2` 后缀；名为空回退 file_key。

### 落点

```
<data_dir>/channels/<channel_name>/files/<message_id>/<file_name>
```

data_dir 由 `kernel.data_dir()` 给出（`~/.yomi`），按消息分组便于
人工翻找与整体清理；不进会话工作目录（会话归属随 mapping 漂移）。

### 占位符就地改写

```
[file: 入园入孵.pptx (key: file_v3_abc)]  →  [file: 入园入孵.pptx (saved: /Users/…/files/om_xxx/入园入孵.pptx)]
```

- 占位符后缀从 `(key: K)` 改为 **`(lark_file_key: K)`**（2026-09-29
  hrli 提议）：`key` 是无主语字符串，agent 看不出用途；平台命名让
  手动兜底自解释——"这是飞书 file_key，用 lark 下载"。mint 点
  （`feishu_text.rs::attachment_placeholder`）与解析点同步改。
- **解析向后兼容**：改写/下载逻辑同时认 `key` 与 `lark_file_key`
  两种后缀——旧 transcript 与 history 里全是旧格式，不能断。
- 只改写触发消息**自身**文本块里的后缀（按 key 精确替换一次）；
  history 注入块里的旧占位符同样按行 msg_id 处理（见下节）。
- 文件名含 `]` 等导致解析失败：不改写、不下载，占位符原样保留
  （行为同今天，无回归）。

### 超限与失败

- **触发消息**：超限或下载失败**不发 reminder、不发说明行**，占位符
  保持 `(lark_file_key: K)` 原样。理由（2026-09-29 hrli 拍板方向）：
  header 自带 `[msg_id: …]`，key 平台命名自解释，agent 自己就能拼出
  `lark im dl <msg_id> <lark_file_key>`——额外提示是重复信息。
- **history 行**：行格式 `[HH:MM] sender: 文本` 不含 msg_id，占位符
  只有 key，手动下载凑不齐参数。此时聚合发**恰好一条**
  `<system_reminder>`：逐文件给出 `name — lark im dl <行msg_id>
  <lark_file_key>`。reminder 语义=补占位符缺的信息，不是重复提示。

### history 里的文件（与图片同规）

history 注入块（`[HH:MM] sender: …` 各行）里的文件占位符**同样下载**：
每行带自己的 `message_id`，逐行解析占位符、按行 msg_id 下载、就地
改写，与 `download_image_pairs` 完全同一姿势。两个硬理由：

1. 不对称会被用户踩到：history 图片现在就能看，文件不能读，"图能
   看图文件不能读"毫无道理。
2. history 行不渲染 msg_id，占位符里的 key 凑不齐手动下载参数，是
   死钥匙——"保持占位符"对该场景不是兜底。

cap 与图片同机制取最新：每触发最多 [`FILE_HISTORY_DOWNLOAD_MAX`]
个（提议 3，图片是 5——文件体积大），超限记 `[N more file(s)
omitted]` 说明行。merge_forward 子消息文件随父 msg_id（与图片现状
一致：端点认就通，不认则与图片同病，不另设特例）。

### 接线点（hub/handlers.rs）

- `ChannelCommand::None`（主流触发）：`msg.content` 文本块先下载改
  写再 extend 进 content。
- `ChannelCommand::Steer`（运行中到达的文件，2026-09-29 事故实际
  走的就是这条）：text 块同样处理。
- `/thread`、`/queue` 命令路径不含文件占位符（命令文本为 `/` 开头
  参数），不接。

## 单文件直发（无伴随文本）

实际最高频的形态：用户不写字、直接丢一个文件进来，正文只有占位符。
本节把该场景的语义钉死，避免实现时特判走样：

1. **与混合消息完全同一条路**：占位符解析、下载、就地改写、超限
   reminder 不做任何单文件特判——`fetch_message_files` 看到的是文本
   块集合，一个块还是「文字 + 占位符」没有区别。
2. **agent 看到的是**：元信息头（`[From User] …[msg_id: …]`）+ 改写后
   的 `[file: name (saved: 路径)]`，除此之外无任何指令。agent 从路径
   `read` 文件内容，按会话既有上下文决定怎么用（填表/摘要/转换…）。
3. **steer 路径必须覆盖**：2026-09-29 事故的实际形态就是运行中到达的
   单文件 steer（PPT、PDF 各一条），`ChannelCommand::Steer` 的接线
   与 `None` 同等重要，测试用例从这里出。
4. **不加"这是用户发来的文件"之类的系统提示**：文件内容自解释，
   v1 不为单文件场景注入额外说明，省 token；若实测发现 agent 对
   裸路径文件消息反应不佳，再评估补一行引导。

## 契约

1. **gate 前零带宽**：下载只发生在消息过闸之后，与
   `ChannelMessage::image_keys` 同一 deferred 纪律。
2. **就地改写是唯一交付**：agent 看到的用户消息里，已下载文件的
   占位符直接给出绝对路径；不再有"自己去某处找文件"的猜谜。
3. **超限不下载、不截断**：超过上限的文件永远不会以半截形式落盘；
  触发消息靠自解释占位符手动兜底，history 行由 reminder 补 msg_id。
4. **reminder 只在 history 缺 msg_id 时发**：一条触发消息无论几个
  history 文件下载不了，只有一条 `<system_reminder>`；触发消息本身
   永不发 reminder。
5. **失败不静默但零噪音**：下载失败在日志（warn!）留痕，消息面不加
   说明行——占位符没被改写成 `(saved: …)` 本身就是"未下载"信号，
   自解释 key + header msg_id 足够 agent 自处。
6. **触发消息与 history 一视同仁**：两处的文件占位符都下载改写；
   唯一不下载的是无 key 的占位符（语法残缺，保留原文）。merge_forward
   子消息文件随父 msg_id，与图片同现状。
7. **其他平台默认 unsupported**：`download_message_file` 默认方法
   返回 unsupported 错误，走到失败说明行路径，telegram 等通道行为
   不变。
8. **cap 常量集中**：`FILE_DOWNLOAD_MAX_BYTES = 100 * 1024 * 1024`
   与 `FILE_HISTORY_DOWNLOAD_MAX = 3` 定义在 hub/context.rs（与
   `IMAGE_DOWNLOAD_MAX` 并列），改阈值单点。

## 测试计划

- `feishu_test.rs`：stub 服务返回文件字节 → 断言落盘内容与路径；
  `Content-Length` 超限 → `Oversize` 且目录无文件；流超限（无
  Content-Length 或服务端多送）→ `Oversize` 且不留半成品；文件名
  含路径成分 → 取 file_name 片段。
- `hub_test.rs`（context）：占位符解析（有/无 name、audio/media
  同类、`]` 畸形输入、旧 `key` 后缀兼容不改写不 panic）；下载成功 →
  文本块就地改写；触发消息超限/失败 → 占位符原样、**无 reminder
  无说明行**；history 行超限 → 恰好一条带行 msg_id 的
  `<system_reminder>`；空 message_id 直接跳过。
- 单文件直发专项：触发消息正文只有占位符时，改写后 agent 可见
  `saved:` 路径；steer 路径同样生效（事故复现形态）。
- history 注入：行内占位符按行 msg_id 下载改写；超 3 个记 omitted
  说明行；无 key 占位符原样不动。
- 回归：`just ci` 全绿；隔离 daemon 真链路 e2e（参考 v0.10.34
  做法：测试账号发文件 → agent 侧文本含 saved 路径 → read 能读；
  补一条 history 场景：文件发在话题早期、后续 @bot 触发，agent
  能从 history 行读到 saved 路径）。

## 决策记录（待拍板）

- 上限 100MB 为拍板值（2026-09-29）；语义"小于下载、大于提示"。
- history 文件每触发下载上限 3 个（图片是 5）为提议值，待拍板。
- 落点 `<data_dir>/channels/<channel>/files/<msg_id>/` 为提议值。
- reminder 用英文（与 `[Failed to download image]` 等既有
  agent-facing 字符串同语言）；如要中文可换。
