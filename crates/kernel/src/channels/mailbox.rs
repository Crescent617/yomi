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

/// 行内容：全量 text（拍平空白，≤300 字符）——preview 的 80 字符在
/// 卡片上太挤看不全。
fn item_text(item: &MailboxItem) -> String {
    let text = item.text.as_deref().unwrap_or(&item.preview);
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 300 {
        format!("{}…", flat.chars().take(299).collect::<String>())
    } else {
        flat
    }
}

/// Pending 卡（info 卡同款蓝头 compact）：每行 kind + 全量文本 + 大号
/// 撤回按钮（primary），底部 steer/queue/全部三个清空按钮；空队列显示
/// 一行占位。
pub(super) fn pending_card(sid: &SessionId, snapshot: &MailboxSnapshot) -> String {
    let items = merged(snapshot);
    let mut elements: Vec<serde_json::Value> = Vec::new();
    if items.is_empty() {
        elements.push(serde_json::json!({
            "tag": "markdown", "text_size": "notation", "content": "Mailbox is empty."
        }));
    }
    for item in items.iter().take(VISIBLE_ROWS) {
        elements.push(serde_json::json!({
            "tag": "column_set",
            "columns": [
                {
                    "tag": "column", "width": "weighted", "weight": 1,
                    "elements": [{ "tag": "markdown", "text_size": "notation", "content": format!("`{}` · {}", kind_label(item), item_text(item)) }],
                },
                {
                    "tag": "column", "width": "auto",
                    "elements": [{
                        "tag": "button",
                        "text": { "tag": "plain_text", "content": "撤回" },
                        "type": "primary",
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
        let button = |scope: MailboxScope, label: &str, kind: &str| {
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
                    "type": kind,
                    "behaviors": [{ "type": "callback", "value": { "action": "mb_clear", "sid": sid.0, "scope": scope_str } }],
                }],
            })
        };
        elements.push(serde_json::json!({
            "tag": "column_set",
            "columns": [
                button(MailboxScope::Steer, "🧹 steer", "default"),
                button(MailboxScope::Queue, "🧹 queue", "default"),
                button(MailboxScope::All, "🧹 全部清空", "danger"),
            ],
        }));
    }
    // 手动刷新：卡片不自动跟踪 mailbox 变化（多卡片并存时注册表难以
    // 维护），需要时用户点一下或重发 `/mailbox`。
    elements.push(serde_json::json!({
        "tag": "column_set",
        "columns": [{
            "tag": "column", "width": "weighted", "weight": 1,
            "elements": [{
                "tag": "button",
                "text": { "tag": "plain_text", "content": "🔄 刷新" },
                "type": "default",
                "behaviors": [{ "type": "callback", "value": { "action": "mb_refresh", "sid": sid.0 } }],
            }],
        }],
    }));
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

/// 按钮回调（`mb_retract` / `mb_clear` / `mb_refresh`）：执行后原地
/// 刷新这张卡片（read-modify-write 后取一次快照——与并发事件交错时
/// 可能不是最新，但 mailbox_changed 事件链保证各端最终收敛）。卡片
/// 不跟踪 mailbox 变化自动刷新：多卡片并存时注册表难维护，需要最新
/// 状态点 🔄 或重发 `/mailbox`。
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
        Some("mb_refresh") => {}
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
