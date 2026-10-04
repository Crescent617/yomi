//! Agent-facing message metadata header (`[k: v]` chain) — the first
//! line of a steered inbound message, identifying who sent it, where,
//! and which message anchors it. Single construction site for all
//! producers (feishu events, telegram, the synthetic new-thread
//! trigger) so the format can't drift apart — the pre-consolidation
//! copies had already drifted once: telegram's header rendered UTC
//! where feishu's rendered local time, 8 hours apart on a UTC+8 host.
//! Doc-comment provenance headers live in `comment.rs` — a different
//! segment set, intentionally not unified here.

use std::fmt::Write;

/// Sanitize a user-controlled display name for the message metadata
/// header: `[`/`]` and control chars become spaces (a newline or bracket
/// could forge header fields), runs of whitespace collapse. `None` when
/// nothing usable remains — callers fall back to the bare-id form.
pub(crate) fn sanitize_header_name(name: &str) -> Option<String> {
    let cleaned = name
        .chars()
        .map(|c| {
            if c == '[' || c == ']' || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The sender segment of a metadata header.
#[derive(Clone, Copy)]
pub(crate) enum HeaderSender<'a> {
    /// A platform user: `[from: name (id)]` when the display name
    /// resolves (sanitized here), else `[from_user_id: id]`.
    User { name: Option<&'a str>, id: &'a str },
    /// A fixed non-user label (internal constant, e.g. the local CLI):
    /// `[from: yomi-cli]`. Not sanitized — caller passes a literal.
    Label(&'a str),
}

/// Inbound-message metadata header:
/// `[{ts}]{from}[chat_id: {chat_id}][msg_id: {msg_id}]{[thread: …]}{[root: …]}[platform: {platform}]`.
pub(crate) fn metadata_header(
    ts: &str,
    sender: HeaderSender<'_>,
    chat_id: &str,
    msg_id: &str,
    thread_id: Option<&str>,
    root_id: Option<&str>,
    platform: &str,
) -> String {
    let from = match sender {
        HeaderSender::User { name, id } => match name.and_then(sanitize_header_name) {
            Some(n) => format!("[from: {n} ({id})]"),
            None => format!("[from_user_id: {id}]"),
        },
        HeaderSender::Label(label) => format!("[from: {label}]"),
    };
    let mut header = format!("[{ts}]{from}[chat_id: {chat_id}][msg_id: {msg_id}]");
    if let Some(tid) = thread_id {
        let _ = write!(header, "[thread: {tid}]");
    }
    if let Some(rid) = root_id {
        let _ = write!(header, "[root: {rid}]");
    }
    let _ = write!(header, "[platform: {platform}]");
    header
}

#[cfg(test)]
#[path = "header_test.rs"]
mod tests;
