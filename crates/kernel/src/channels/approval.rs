//! Feishu cloud-document permission applications: notification cards and
//! command/button approvals.
//! See `docs/design/feishu-doc-permission-approval.md`.

use super::{
    CardAction, ChannelConfig, ChannelStore, DocPermissionRequest, PermRequestRow, PlatformAdapter,
};
use crate::types::{ContentBlock, Result as KernelResult};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Permission levels grantable through `/approve` (button approvals always
/// grant the requested level).
const PERM_LEVELS: &[&str] = &["view", "edit", "full_access"];

/// Usage text for malformed approval commands.
const APPROVAL_USAGE: &str = "\
用法：
`/permits` — 列出待审批申请
`/approve <id> [view|edit|full_access]` — 批准（可改权限级别）
`/deny <id>` — 拒绝";

/// `/permits` shows at most this many rows (oldest first) to keep the
/// reply card well under the platform payload cap.
const MAX_PENDING_ROWS: usize = 50;

// ── Inbound event ──────────────────────────────────────────────────

/// Persist a doc-permission application (deduplicated) and deliver the
/// notification card: to `approval_chat_id` when configured, else by DM
/// to every admin. Per-recipient failures don't take down the rest — the
/// stored row keeps the request approvable via `/permits`.
pub(super) async fn handle_doc_permission_applied(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    req: DocPermissionRequest,
) {
    let id = match store.save_perm_request(channel_name, &req).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            debug!(channel = %channel_name, "duplicate doc permission request skipped");
            return;
        }
        Err(e) => {
            error!(channel = %channel_name, error = %e, "failed to record doc permission request");
            return;
        }
    };
    info!(channel = %channel_name, id, "doc permission request recorded");

    let doc_title = adapter
        .fetch_doc_title(&req.file_token, &req.file_type)
        .await;
    let card = build_request_card(id, &req, doc_title.as_deref());
    let mut msg_ids = Vec::new();
    if let Some(chat_id) = config.approval_chat_id.as_deref() {
        match adapter.send_card(chat_id, &card, None).await {
            Ok(Some(mid)) => msg_ids.push(mid),
            Ok(None) => {
                warn!(channel = %channel_name, id, "approval card sent without message id");
            }
            Err(e) => {
                error!(channel = %channel_name, id, error = %e, "failed to send approval card");
            }
        }
    } else if config.admin_users.is_empty() {
        info!(
            channel = %channel_name,
            id, "no approval_chat_id or admin_users; request recorded only"
        );
    } else {
        for admin in &config.admin_users {
            match adapter.send_direct_card(admin, &card).await {
                Ok(Some(mid)) => msg_ids.push(mid),
                Ok(None) => {}
                Err(e) => {
                    warn!(channel = %channel_name, id, admin = %admin, error = %e, "failed to DM approval card");
                }
            }
        }
        if msg_ids.is_empty() {
            error!(channel = %channel_name, id, "approval card reached no admin");
        }
    }

    if !msg_ids.is_empty() {
        if let Err(e) = store.set_perm_notify_msgs(id, &msg_ids).await {
            warn!(channel = %channel_name, id, error = %e, "failed to record notify msg ids");
        }
    }
}

// ── Commands ───────────────────────────────────────────────────────

/// `/permits` — list this channel's pending applications (admin only).
pub(super) async fn list_pending(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    user_id: &str,
) -> KernelResult<Option<String>> {
    if let Some(reply) = check_admin(config, user_id) {
        return Ok(Some(reply));
    }
    let rows = store.list_pending_perm_requests(channel_name).await?;
    Ok(Some(format_pending_list(&rows)))
}

/// `/approve <id> [perm]` — grant the requested (or overridden) level.
pub(super) async fn approve(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    user_id: &str,
    id: i64,
    perm: Option<&str>,
) -> KernelResult<Option<String>> {
    if let Some(reply) = check_admin(config, user_id) {
        return Ok(Some(reply));
    }
    if let Some(perm) = perm {
        if !PERM_LEVELS.contains(&perm) {
            return Ok(Some(format!(
                "无效权限级别 `{perm}`，可选：{}",
                PERM_LEVELS.join("/")
            )));
        }
    }
    let effective = resolve(
        channel_name,
        store,
        adapter,
        user_id,
        id,
        Resolution::Approved(perm),
    )
    .await?;
    Ok(Some(effective.text))
}

/// `/deny <id>` — mark denied locally; Feishu has no reject API.
pub(super) async fn deny(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    user_id: &str,
    id: i64,
) -> KernelResult<Option<String>> {
    if let Some(reply) = check_admin(config, user_id) {
        return Ok(Some(reply));
    }
    let reply = resolve(
        channel_name,
        store,
        adapter,
        user_id,
        id,
        Resolution::Denied,
    )
    .await?;
    Ok(Some(reply.text))
}

/// Usage error reply for malformed approval commands.
pub(super) fn usage() -> String {
    APPROVAL_USAGE.to_string()
}

// ── Card button callbacks ──────────────────────────────────────────

/// Button tap on a notification card (`card.action.trigger`): the value
/// carries `{"action": "approve"|"deny", "id": N}`. Successful resolutions
/// speak through the updated terminal-state cards; only failures and
/// rejections get an extra chat message in the callback's chat.
pub(super) async fn handle_card_action(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: CardAction,
) {
    let chat_id = action.chat_id.clone();
    match handle_card_action_inner(channel_name, config, store, adapter, action).await {
        Ok(Some(reply)) if !reply.success || !reply.cards_updated => {
            let Some(chat_id) = chat_id else {
                warn!(channel = %channel_name, text = %reply.text, "card action failed without chat context");
                return;
            };
            let adapter = Arc::clone(adapter);
            let text = reply.text;
            tokio::spawn(async move {
                if let Err(e) = adapter
                    .send_message(&chat_id, vec![ContentBlock::Text { text }], None)
                    .await
                {
                    warn!(error = %e, "failed to send card action feedback");
                }
            });
        }
        Ok(_) => {}
        Err(e) => error!(channel = %channel_name, error = %e, "card action failed"),
    }
}

async fn handle_card_action_inner(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: CardAction,
) -> KernelResult<Option<ApprovalReply>> {
    let action_type = action.value["action"].as_str().unwrap_or("");
    let id = action.value["id"].as_i64().unwrap_or(-1);
    if id <= 0 || !matches!(action_type, "approve" | "deny") {
        warn!(value = %action.value, "unrecognized card action value");
        return Ok(None);
    }
    if let Some(reply) = check_admin(config, &action.operator_open_id) {
        return Ok(Some(ApprovalReply {
            text: reply,
            success: false,
            cards_updated: false,
        }));
    }
    let resolution = if action_type == "approve" {
        Resolution::Approved(None)
    } else {
        Resolution::Denied
    };
    let reply = resolve(
        channel_name,
        store,
        adapter,
        &action.operator_open_id,
        id,
        resolution,
    )
    .await?;
    Ok(Some(reply))
}

// ── Shared resolve path ────────────────────────────────────────────

/// A resolution decision: approve with the requested (or an overridden)
/// level, or deny.
enum Resolution<'a> {
    Approved(Option<&'a str>),
    Denied,
}

/// Command/button feedback. Successful resolutions are visible on the
/// updated cards themselves; failures — and successes with no card left
/// to show (notification writes failed earlier) — need an explicit
/// message.
struct ApprovalReply {
    text: String,
    success: bool,
    /// Whether any notification card was updated to the terminal state.
    cards_updated: bool,
}

/// Shared approve/deny path for commands and buttons: win the resolve
/// race (exactly once), then act. Approval grants via the platform API —
/// a grant failure reopens the request so nothing is lost; an
/// unrecognized stored level (R6) also reopens, demanding an explicit
/// `/approve <id> <perm>` instead.
async fn resolve(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    operator: &str,
    id: i64,
    resolution: Resolution<'_>,
) -> KernelResult<ApprovalReply> {
    let (status, resolved_perm) = match resolution {
        Resolution::Approved(perm) => ("approved", perm),
        Resolution::Denied => ("denied", None),
    };
    let Some(row) = store
        .resolve_perm_request(id, status, operator, resolved_perm)
        .await?
    else {
        return Ok(ApprovalReply {
            text: format!("申请 #{id} 不存在或已被处理"),
            success: false,
            cards_updated: false,
        });
    };

    if status == "approved" {
        let effective_perm = row
            .resolved_perm
            .as_deref()
            .unwrap_or(&row.permission)
            .to_string();
        if !PERM_LEVELS.contains(&effective_perm.as_str()) {
            warn!(channel = %channel_name, id, perm = %effective_perm, "unrecognized requested level, reopening");
            if let Err(e) = store.reopen_perm_request(id).await {
                error!(channel = %channel_name, id, error = %e, "failed to reopen perm request");
            }
            return Ok(ApprovalReply {
                text: format!(
                    "申请权限 `{effective_perm}` 无法识别，未执行授权。请用 `/approve {id} [view|edit|full_access]` 显式指定级别。"
                ),
                success: false,
                cards_updated: false,
            });
        }
        let req = request_from_row(&row);
        if let Err(e) = adapter
            .grant_doc_permission(&row.file_token, &row.file_type, &req, &effective_perm)
            .await
        {
            error!(channel = %channel_name, id, error = %e, "doc permission grant failed, reopening");
            if let Err(e) = store.reopen_perm_request(id).await {
                error!(channel = %channel_name, id, error = %e, "failed to reopen perm request");
            }
            return Ok(ApprovalReply {
                text: format!("批准 #{id} 失败：{e}。申请已恢复为待审批，可重试。"),
                success: false,
                cards_updated: false,
            });
        }
        let cards_updated = update_notify_cards(adapter, &row).await;
        return Ok(ApprovalReply {
            text: format!(
                "已批准 #{id}：{} 获得 {effective_perm} 权限",
                applicant_summary(&row)
            ),
            success: true,
            cards_updated,
        });
    }

    let cards_updated = update_notify_cards(adapter, &row).await;
    Ok(ApprovalReply {
        text: format!("已拒绝 #{id}：{}", applicant_summary(&row)),
        success: true,
        cards_updated,
    })
}

/// Rewrite every notification card to its terminal state (the group card
/// or the per-admin DM cards alike); returns whether any were updated.
async fn update_notify_cards(adapter: &Arc<dyn PlatformAdapter>, row: &PermRequestRow) -> bool {
    let doc_title = adapter
        .fetch_doc_title(&row.file_token, &row.file_type)
        .await;
    let card = build_resolved_card(row, doc_title.as_deref());
    let mut any = false;
    for msg_id in &row.notify_msg_ids {
        match adapter.update_card(msg_id, &card).await {
            Ok(()) => any = true,
            Err(e) => {
                warn!(id = row.id, msg_id = %msg_id, error = %e, "failed to update notify card");
            }
        }
    }
    any
}

pub(super) fn check_admin(config: &ChannelConfig, user_id: &str) -> Option<String> {
    if config.admin_users.iter().any(|u| u == user_id) {
        None
    } else {
        Some("permission denied：你不在 admin_users 中。".to_string())
    }
}

fn request_from_row(row: &PermRequestRow) -> DocPermissionRequest {
    DocPermissionRequest {
        file_token: row.file_token.clone(),
        file_type: row.file_type.clone(),
        permission: row.permission.clone(),
        remark: row.remark.clone(),
        applicant_users: row.applicant_users.clone(),
        applicant_chats: row.applicant_chats.clone(),
        applicant_departments: row.applicant_departments.clone(),
    }
}

// ── Formatting ─────────────────────────────────────────────────────

/// One-line applicant summary: users as `<at>` mentions (render as
/// clickable names on Feishu cards), first user only when several;
/// chats and departments as plain text. Joined by " · ".
fn format_applicants(users: &[String], chats: &[String], departments: &[String]) -> String {
    let mut parts = Vec::new();
    if let Some(first) = users.first() {
        let mention = format!("<at id={first}></at>");
        if users.len() == 1 {
            parts.push(mention);
        } else {
            parts.push(format!("{}（等 {} 人）", mention, users.len()));
        }
    }
    parts.extend(chats.iter().map(|c| format!("群 {c}")));
    parts.extend(departments.iter().map(|d| format!("部门 {d}")));
    parts.join(" · ")
}

fn applicant_summary(row: &PermRequestRow) -> String {
    format_applicants(
        &row.applicant_users,
        &row.applicant_chats,
        &row.applicant_departments,
    )
}

/// Markdown for the linked document reference, shared by cards and lists.
fn doc_md(file_type: &str, file_token: &str) -> String {
    format!(
        "[{file_type}/{file_token}]({})",
        super::doc_link(file_type, file_token)
    )
}

fn format_pending_list(rows: &[PermRequestRow]) -> String {
    if rows.is_empty() {
        return "没有待审批的文档权限申请。".to_string();
    }
    let mut lines = vec![
        "**待审批的文档权限申请**（批准 `/approve <id> [perm]` · 拒绝 `/deny <id>`）".to_string(),
        String::new(),
    ];
    for row in rows.iter().take(MAX_PENDING_ROWS) {
        lines.push(format!(
            "- `#{}` {} · {} · 申请 {} · {}",
            row.id,
            applicant_summary(row),
            doc_md(&row.file_type, &row.file_token),
            row.permission,
            row.created_at,
        ));
    }
    if rows.len() > MAX_PENDING_ROWS {
        lines.push(format!(
            "… 共 {} 条，仅显示前 {MAX_PENDING_ROWS} 条",
            rows.len()
        ));
    }
    lines.join("\n")
}

/// Compact schema-2.0 card, mirroring the status-card layout.
fn card_json(template: &str, title: &str, elements: &[serde_json::Value]) -> String {
    json!({
        "schema": "2.0",
        "config": { "width_mode": "compact" },
        "header": {
            "title": { "tag": "plain_text", "content": title },
            "template": template,
            "padding": "4px 12px 4px 12px",
        },
        "body": {
            "padding": "8px 12px 8px 12px",
            "elements": elements,
        },
    })
    .to_string()
}

/// Pending-state notification card with approve/deny buttons. The button
/// values carry the request id; approve grants the requested level (level
/// overrides stay with the `/approve` command).
fn build_request_card(id: i64, req: &DocPermissionRequest, doc_title: Option<&str>) -> String {
    let doc_text = doc_title.unwrap_or(&req.file_token);
    let mut lines = vec![
        format!(
            "**申请人** {}",
            format_applicants(
                &req.applicant_users,
                &req.applicant_chats,
                &req.applicant_departments
            )
        ),
        format!(
            "**文档** [{}]({})",
            doc_text,
            super::doc_link(&req.file_type, &req.file_token)
        ),
        format!("**申请权限** {}", req.permission),
    ];
    if let Some(remark) = req.remark.as_deref().filter(|r| !r.is_empty()) {
        lines.push(format!("**备注** {remark}"));
    }

    card_json(
        "orange",
        &format!("📄 文档权限申请 #{id}"),
        &[
            json!({ "tag": "markdown", "text_size": "notation", "content": lines.join("\n") }),
            // Schema 2.0 dropped the `action` container tag — buttons are
            // body elements in their own right (verified: API error 200861);
            // a column_set puts them on one row.
            json!({
                "tag": "column_set",
                "columns": [
                    {
                        "tag": "column",
                        "width": "weighted",
                        "weight": 1,
                        "elements": [{
                            "tag": "button",
                            "text": { "tag": "plain_text", "content": "✅ 批准" },
                            "type": "primary",
                            "behaviors": [{ "type": "callback", "value": { "action": "approve", "id": id } }],
                        }],
                    },
                    {
                        "tag": "column",
                        "width": "weighted",
                        "weight": 1,
                        "elements": [{
                            "tag": "button",
                            "text": { "tag": "plain_text", "content": "❌ 拒绝" },
                            "type": "danger",
                            "behaviors": [{ "type": "callback", "value": { "action": "deny", "id": id } }],
                        }],
                    },
                ],
            }),
            json!({
                "tag": "markdown",
                "text_size": "notation",
                "content": format!("<font color='grey'>改权限批准：`/approve {id} [view|edit|full_access]`</font>"),
            }),
        ],
    )
}

/// Terminal-state card (no buttons): approved in green, denied in grey.
fn build_resolved_card(row: &PermRequestRow, doc_title: Option<&str>) -> String {
    let approved = row.status == "approved";
    let (template, mark, action_text) = if approved {
        let perm = row.resolved_perm.as_deref().unwrap_or(&row.permission);
        ("green", "✅", format!("已批准 {perm}"))
    } else {
        ("grey", "❌", "已拒绝".to_string())
    };
    let by = row.resolved_by.as_deref().unwrap_or("unknown");
    let by_text = if by.starts_with("ou_") {
        format!("<at id={by}></at>")
    } else {
        by.to_string()
    };
    let doc_text = doc_title.unwrap_or(&row.file_token);
    let content = format!(
        "**申请人** {}\n**文档** [{}]({})\n\n**{action_text}** · by {by_text}",
        applicant_summary(row),
        doc_text,
        super::doc_link(&row.file_type, &row.file_token),
    );
    card_json(
        template,
        &format!("{mark} 文档权限申请 #{}", row.id),
        &[json!({ "tag": "markdown", "text_size": "notation", "content": content })],
    )
}

#[cfg(test)]
#[path = "approval_test.rs"]
mod tests;
