# 图片附件内嵌回复卡（inline image attachments）

飞书通道的 `<yomi_attachments>` 图片附件不再一律事后单独发消息：在
回复落地前预上传拿 `image_key`，以 `img` 元素嵌进回复卡——**块的
位置就是图片的位置**：块写在正文哪段话后面，图片就出现在哪；块在
文末则统一附在正文之后（兼容旧习惯）。非图片附件维持事后逐条发
文件消息。

## 位置机制（锚定 token）

回复缓冲记录时（`record_model_end`），每个块在原地为每个**图片**
路径留下一个占位 token（`⟦yomi-attachment:<声明路径>⟧`），块本身
照常剥离、路径照常收集。token 自带声明路径、单行、对所有后续文本
变换（正文提升拼接、@人改写）免疫——全系统只有两个端点要懂它：
卡片渲染（按 token 切开正文插入 img 元素）与纯文本面（剥掉）。两
个铸造期守卫：源文本里既有的字面 token（模型引用该格式）统一在
括号后加一个空格中和掉（字形不变、模式失效，绝不被当活锚点）；声
明路径本身含 `⟦`/`⟧` 的不铸 token（会提前截断，图片退为正文后
统一放）。会话落盘原文不受影响；GUI 用不带锚定的剥离，解析规则
完全一致（其 TS port 同步注释照旧）。

## 契约

1. **一条 run 仍是一条消息**：图片随回复卡走（morph 与 flush 两条
   卡路径同规，`render_card` 统一渲染），不产生额外消息、不额外
   通知。
2. **位置语义只对正式正文生效**：中间叙述（trace 过程面板）里的
   token 一律剥除，其中声明的图片按"正文后统一放"处理。
3. **只覆盖图片**：飞书卡片没有文件类模块，非图片附件（pdf/xlsx/
   zip…）维持事后逐条发文件消息，落在回复之后；写进内联位的文件
   同样回该路径。
4. **降级不丢图**：平台无 upload-only 接口（`upload_image` 默认
   `Ok(None)`，如 telegram）、预上传失败、run 无正文文本、token
   被超长截断切掉（半个 token 剥除）——对应图片一律回到"正文后
   统一放"或事后文件路径，失败原因由该路径照旧暴露。
5. **纯文本兜底不丢图**：`flush_reply` 落到无卡纯文本路径时，内嵌
   图片经 `send_attachments` 转为正文之后补发文件消息，失败补发
   说明，不静默。
6. **时序代价**：预上传发生在 settle/flush 之前，每张图增加一个
   上传 RTT；串行实现（附件量级小，上传时延本来就随回复走）。
7. **歧义失败可能重复**：卡片发送/PATCH 报错但平台实际已收（应答
   丢失）时，纯文本兜底会重发正文并把内嵌图片补发为文件消息——
   正文与图片都重复。该重复语义正文本就有，图片继承，不另设
   去重。
8. **纯图回复**：正文只有图片块时正文面上渲染为单图卡（无
   markdown 元素）；纯文本/无卡路径不落空气泡（平台会拒收空
   文本），图片经事后文件路径交付。中间叙述里的纯图 token 在
   trace 行与过程面板中跳过，不留空元素。
9. **声明去重与锚定拼写**：同一文件以不同拼写重复声明（`a.png`
   与 `./a.png`）按解析后路径去重，锚定匹配用首个声明串；正文
   token 若写另一拼写则匹配不上，图片退化为正文后统一放——是
   降级不是丢图，不另做拼写归一。
10. **围栏口径**：声明解析的围栏判定只认 ` ``` ` 标记行（与
    `map_outside_fences` 一致）；`~~~` 围栏里的块按声明处理，卡
    片渲染（CommonMark 口径）可能把图片切进代码块——存量解析口
    径，概率低，不另改。

## 实现对应

| 环节 | 位置 |
|---|---|
| 锚定解析 / token 工具 | `utils/attachments.rs`（`parse_attachments_anchored`、`strip_attachment_tokens`、`strip_dangling_token`） |
| 平台 upload-only 接口 | `PlatformAdapter::upload_image`（channels/mod.rs），飞书实现拆自 `send_one_file` 的 `/im/v1/images` 上传（不发送） |
| 解析与分流 | `attachments::resolve_attachments`（declared→resolved 对）、`inline_partition_for_reply` |
| 回复携带句柄 | `FinalReply::inline_images`（key + alt + 源路径 + declared） |
| 卡片渲染 | `render/reply.rs::render_card`（token 切分 + 截断清理 + 尾部兜底） |
| 接线 | `hub/deliver.rs::deliver_reply`（解析后预上传）、`flush_reply`（无卡兜底转文件消息） |
| agent 契约 | prompt `ATTACHMENTS_SECTION`：块的位置=图片位置，文件始终事后发 |
