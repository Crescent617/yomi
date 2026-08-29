//! 平台适配层：feishu（全能力：卡片/话题/订阅/文档评论）与 telegram
//! （纯文本）。各自由 kernel 同名 cargo feature 控制编译（默认
//! `all-channels` 全开）；编译外的平台仍保留 config 类型，仅适配器
//! 构造报错（见 `hub::build_adapter`）。

#[cfg(feature = "feishu")]
pub(crate) mod feishu;
#[cfg(feature = "feishu")]
pub(crate) mod feishu_events;
#[cfg(feature = "feishu")]
pub(crate) mod feishu_text;
#[cfg(feature = "telegram")]
pub(crate) mod telegram;
