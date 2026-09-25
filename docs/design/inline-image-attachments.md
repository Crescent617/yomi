# 图片附件内嵌回复卡（inline image attachments）

飞书通道的 `<yomi_attachments>` 图片附件不再一律事后单独发消息：在
回复落地前预上传拿 `image_key`，以 `img` 元素嵌在回复卡正文之下、
trace 面板之上——图和文在同一条消息里。

## 契约

1. **一条 run 仍是一条消息**：图片随回复卡走（morph 与 flush 两条
   卡路径同规，`render_card` 统一渲染），不产生额外消息、不额外
   通知。
2. **只覆盖图片**：飞书卡片没有文件类模块，非图片附件（pdf/xlsx/
   zip…）维持事后逐条发文件消息，落在回复之后。
3. **回退一律向保守方向**：平台无 upload-only 接口（`upload_image`
   默认 `Ok(None)`，如 telegram）、文件非图片、预上传失败、或 run
   没有正文文本（没有可依附的卡）——该文件都回到事后文件路径，
   失败原因由该路径照旧暴露，不静默丢件。
4. **纯文本兜底不丢图**：`flush_reply` 落到无卡纯文本路径时，内嵌
   图片转为正文之后补发文件消息（`InlineImage` 保留源路径就是为
   这一跳）。
5. **时序代价**：预上传发生在 settle/flush 之前，每张图增加一个
   上传 RTT；串行实现（附件量级小，上传时延本来就随回复走）。
6. **歧义失败可能重复**：卡片发送/PATCH 报错但平台实际已收（应答
   丢失）时，纯文本兜底会重发正文并把内嵌图片补发为文件消息——
   正文与图片都重复。该重复语义正文本就有，图片继承，不另设
   去重。
7. **声明语法不变**：agent 侧契约（prompt 的 attachments 节）不改，
   "delivered as an attachment alongside your message" 对内嵌图
   片依然成立。

## 实现对应

| 环节 | 位置 |
|---|---|
| 平台 upload-only 接口 | `PlatformAdapter::upload_image`（channels/mod.rs），飞书实现拆自 `send_one_file` 的 `/im/v1/images` 上传（不发送） |
| 图片/文件分流 | `attachments::inline_partition` |
| 回复携带句柄 | `FinalReply::inline_images`（key + alt + 源路径） |
| 卡片渲染 | `render/reply.rs::render_card` 正文之后插 `img` 元素 |
| 接线 | `hub/deliver.rs::deliver_reply`（解析后预上传）、`flush_reply`（无卡兜底转文件消息） |
