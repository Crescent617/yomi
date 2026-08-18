//! `/mailbox` — kernel mailbox 的通道管理面：pending 条目以卡片呈现
//! （每行带撤回按钮，底部按范围清空），按钮回调原地刷新；无卡片能力
//! 的平台退回文本 + `/mailbox retract <n>` / `/mailbox clear`。
//! 预览会泄露消息内容，命令与按钮都限 admin（与 `/sessions` 同档）。

use std::sync::Arc;
use tracing::warn;

use crate::comms::{MailboxItem, MailboxScope, MailboxSnapshot};
use crate::kernel::Kernel;
use crate::types::{Result as KernelResult, SessionId};

use super::hub_deliver::info_card_envelope;
use super::{CardAction, ChannelConfig, PlatformAdapter};

/// 卡片/文本可见行数；溢出折叠成提示。
const VISIBLE_ROWS: usize = 8;

/// `/mailbox` 的子操作（命令解析产物）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailboxSub {
    Show,
    Clear(MailboxScope),
    Retract(usize),
}

fn kind_label(item: &MailboxItem) -> &'static str {
    match item.kind {
        crate::comms::MailboxItemKind::Steer => "↳ steer",
        crate::comms::MailboxItemKind::Queue => "⏱ queue",
    }
}

fn merged(snapshot: &MailboxSnapshot) -> Vec<&MailboxItem> {
    snapshot.steer.iter().chain(snapshot.queue.iter()).collect()
}

fn parse_scope(v: &serde_json::Value) -> MailboxScope {
    match v.as_str() {
        Some("steer") => MailboxScope::Steer,
        Some("queue") => MailboxScope::Queue,
        _ => MailboxScope::All,
    }
}

/// Pending 卡（info 卡同款蓝头 compact）：每行 kind + preview + 撤回按钮，
/// 底部 steer/queue/全部三个清空按钮；空队列显示一行占位。
pub(super) fn pending_card(sid: &SessionId, snapshot: &MailboxSnapshot) -> String {
    let items = merged(snapshot);
    let mut elements: Vec<serde_json::Value> = Vec::new();
    if items.is_empty() {
        elements.push(serde_json::json!({
            "tag": "markdown", "content": "Mailbox is empty."
        }));
    }
    for item in items.iter().take(VISIBLE_ROWS) {
        elements.push(serde_json::json!({
            "tag": "column_set",
            "columns": [
                {
                    "tag": "column", "width": "weighted", "weight": 1,
                    "elements": [{ "tag": "markdown", "content": format!("`{}` · {}", kind_label(item), item.preview) }],
                },
                {
                    "tag": "column", "width": "auto",
                    "elements": [{
                        "tag": "button",
                        "text": { "tag": "plain_text", "content": "撤回" },
                        "type": "default",
                        "behaviors": [{ "type": "callback", "value": { "action": "mb_retract", "sid": sid.0, "item": item.id } }],
                    }],
                },
            ],
        }));
    }
    let overflow = items.len().saturating_sub(VISIBLE_ROWS);
    if overflow > 0 {
        elements.push(serde_json::json!({
            "tag": "markdown", "text_size": "notation",
            "content": format!("<font color='grey'>… and {overflow} more — 用下方按钮清空</font>"),
        }));
    }
    if !items.is_empty() {
        let button = |scope: MailboxScope, label: &str| {
            let scope_str = match scope {
                MailboxScope::Steer => "steer",
                MailboxScope::Queue => "queue",
                MailboxScope::All => "all",
            };
            serde_json::json!({
                "tag": "column", "width": "weighted", "weight": 1,
                "elements": [{
                    "tag": "button",
                    "text": { "tag": "plain_text", "content": label },
                    "type": "default",
                    "behaviors": [{ "type": "callback", "value": { "action": "mb_clear", "sid": sid.0, "scope": scope_str } }],
                }],
            })
        };
        elements.push(serde_json::json!({
            "tag": "column_set",
            "columns": [
                button(MailboxScope::Steer, "🧹 steer"),
                button(MailboxScope::Queue, "🧹 queue"),
                button(MailboxScope::All, "🧹 全部"),
            ],
        }));
    }
    info_card_envelope(&format!("⏳ Pending ({})", items.len()), elements)
}

/// 文本回退（Telegram / 文档评论）。
pub(super) fn pending_text(snapshot: &MailboxSnapshot) -> String {
    let items = merged(snapshot);
    let mut lines = Vec::new();
    for item in items.iter().take(VISIBLE_ROWS) {
        lines.push(format!("- `{}` · {}", kind_label(item), item.preview));
    }
    let overflow = items.len().saturating_sub(VISIBLE_ROWS);
    if overflow > 0 {
        lines.push(format!("… and {overflow} more"));
    }
    lines.push(String::new());
    lines.push(
        "管理：`/mailbox retract <序号>` 撤回 · `/mailbox clear [steer|queue]` 清空".to_string(),
    );
    lines.join("\n")
}

/// 按钮回调（`mb_retract` / `mb_clear`）：执行后尽量原地刷新卡片
/// （read-modify-write 后刷新一次快照——与并发事件交错时可能不是最
/// 新，但 mailbox_changed 事件链保证各端最终收敛；与 approval 的
/// read-then-write 同级假设）；拿不到消息 id 时静默（用户可 `/mailbox`
/// 再看）；失败回一条反馈。
pub(super) async fn handle_card_action(
    channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: CardAction,
) {
    if let Err(e) = handle_card_action_inner(channel_name, config, kernel, adapter, &action).await {
        warn!(channel = %channel_name, error = %e, "mailbox card action failed");
    }
}

async fn handle_card_action_inner(
    _channel_name: &str,
    config: &ChannelConfig,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: &CardAction,
) -> KernelResult<()> {
    let value = &action.value;
    let sid = SessionId::from(value["sid"].as_str().unwrap_or_default().to_string());
    if sid.0.is_empty() {
        warn!(value = %value, "mailbox card action missing sid");
        return Ok(());
    }
    if let Some(deny) = super::approval::check_admin(config, &action.operator_open_id) {
        if let Some(chat_id) = &action.chat_id {
            let _ = adapter
                .send_message(
                    chat_id,
                    vec![crate::types::ContentBlock::Text { text: deny }],
                    None,
                )
                .await;
        }
        return Ok(());
    }
    match value["action"].as_str() {
        Some("mb_retract") => {
            let item = value["item"].as_str().unwrap_or_default();
            kernel.remove_mailbox_item(&sid, item).await;
        }
        Some("mb_clear") => {
            kernel
                .clear_mailbox(&sid, parse_scope(&value["scope"]))
                .await;
        }
        other => {
            warn!(value = %value, "unrecognized mailbox card action {other:?}");
            return Ok(());
        }
    }
    if let Some(message_id) = &action.message_id {
        let snapshot = kernel.mailbox_snapshot(&sid).await;
        adapter
            .update_card(message_id, &pending_card(&sid, &snapshot))
            .await?;
    }
    Ok(())
}

/// `/mailbox` 命令主体（admin 门槛在命令臂，此处只管执行）。
pub(super) async fn handle_mailbox_command(
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &super::ChannelMessage,
    reply_msg_id: Option<String>,
    sid: &SessionId,
    sub: MailboxSub,
) -> KernelResult<Option<String>> {
    match sub {
        MailboxSub::Show => {
            let snapshot = kernel.mailbox_snapshot(sid).await;
            if merged(&snapshot).is_empty() {
                return Ok(Some("Mailbox is empty.".to_string()));
            }
            if msg.doc_comment.is_none() && adapter.supports_status_card() {
                adapter
                    .send_card(
                        &msg.external_chat_id,
                        &pending_card(sid, &snapshot),
                        reply_msg_id.as_deref(),
                    )
                    .await?;
                return Ok(None);
            }
            Ok(Some(pending_text(&snapshot)))
        }
        MailboxSub::Clear(scope) => {
            let removed = kernel.clear_mailbox(sid, scope).await;
            Ok(Some(format!("🧹 Cleared {removed} pending item(s).")))
        }
        MailboxSub::Retract(n) => {
            let snapshot = kernel.mailbox_snapshot(sid).await;
            let items = merged(&snapshot);
            let Some(item) = n.checked_sub(1).and_then(|i| items.get(i)) else {
                return Ok(Some(format!(
                    "No pending item #{n} ({} item(s) in the mailbox).",
                    items.len()
                )));
            };
            let id = item.id.clone();
            if kernel.remove_mailbox_item(sid, id.as_str()).await {
                Ok(Some(format!("Retracted #{n}: {}", item.preview)))
            } else {
                Ok(Some(format!("#{n} is already gone (consumed).")))
            }
        }
    }
}
