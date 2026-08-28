# 决策记录：移除内建 `web_fetch` 工具

## 决策

内建 `web_fetch` 工具已移除（kernel 实现、权限表项、GUI/TUI 展示映射）。
网页抓取走 **shell curl + 转换工具**（pandoc / macOS 自带 textutil / pup），
模型自行把内容落盘为文件，再用 `read`/`grep` 按需翻阅。

## 理由

1. **零内核耦合**：`web_fetch` 是纯 URL→text 函数，不碰 FileStateStore、
   checkpoint、session 等任何内核状态。内建工具应该是必须长在核心里
   的东西，它不符合四端口哲学中 capability 端口「默认 out-of-proc」的定位。
2. **截断即数据丢失**：工具把内容憋在 10K 返回值里，补偿方案（内核磁盘缓存）
   是在给紧身衣打补丁。删掉工具后，40K 工具输出上限自然逼模型 `curl -o`
   落盘——「全量留文件 + 按需翻阅」从被设计的机制变成涌现行为（一切皆文件）。
3. **提取质量是站点相关的**：单一手搓提取器（main/article 选择器 +
   空白压平）不可能覆盖 web 的多样性；模型按站点选 pandoc/textutil/pup/
   readability 更优，且随模型能力免费升级。
4. **安全面无回归**：curl 能 exfiltrate（`-d @file`）的面 web_fetch 天然没有，
   但 yolo 部署下门本来就开着；默认配置下 shell（Caution）反而比
   web_fetch（Safe）管得更严——方向符合「权限只收窄」。

## 代价（接受）

- 非 yolo 配置下抓取从免审批（Safe）变为需审批（shell Caution），
  包括 cron 无人值守场景。
- 重复抓取的 TTL 去重从机制降为约定（skill/习惯用 mtime 判断新鲜度）。
- 若未来做 taint 标记（不可信内容追踪），web_fetch 本是天然挂点，
  需在 shell 层另找方案。

## 注

- `utils/html.rs` 保留：`web_search` 的结果提取仍在用。
- `file://` 读取能力随工具移除，本就与 `read` 工具重复。
- 本文件前身是「web_fetch 磁盘缓存与提取质量改造」设计稿（P1-P3），
  随本决策废弃——那个方向是在修补工具的紧身衣，而非质疑紧身衣本身。
