//! Inbound message reactions (Feishu `im.message.reaction.created_v1`):
//! a user's emoji reaction as session input. Two modes by the target
//! chat's mapping kind:
//!
//! - **Normal chats**: feedback steered into the RUNNING session of the
//!   reacted message's conversation — never a trigger. An idle session
//!   is NOT woken (every 👍 spawning a run would burn a model call per
//!   emoji), so reactions arriving after the run settles are dropped.
//! - **Watch chats** (`/watch on`, chat-level mapping): reactions are
//!   observations like any mirrored message — batched into the watch
//!   mirror queue and delivered on the next window flush, never
//!   dropped, and the user-allowlist miss bypass applies (a watched
//!   chat mirrors the whole conversation, same as `gate.rs`).
//!
//! The whole feature is one `disabled_events = ["reaction"]` away from
//! silence, and inert on platforms whose adapter doesn't implement
//! `fetch_reaction_target` (the default returns `None`).
//!
//! No receipts, no obs, no history cursor: a reaction is not a posted
//! message — recording one as a receipt would falsely trigger mid-run
//! split (the status card must not freeze because someone added an
//! emoji). One race is accepted (normal path only): the run may settle
//! between the running-check and the steer publish — a steer to a
//! just-idled session starts a run that sees only the reaction. The
//! window is milliseconds; the outcome is one cheap no-op turn.

use std::sync::{Arc, Weak};
use tracing::{debug, info, warn};

use crate::kernel::Kernel;
use crate::types::{ContentBlock, SessionId};

use crate::channels::header::{metadata_header, HeaderSender};
use crate::channels::{
    AccessDeniedReason, ChannelConfig, ChannelError, ChannelMessage, ChannelStore, PlatformAdapter,
    ReactionNotice, ReactionTarget,
};

/// Handle one reaction event. Runs off the gate loop (spawned): the
/// target lookup costs one platform GET and must not stall chat
/// processing. Policy checks come first — a disabled feature or a
/// denied operator pays zero platform API calls.
pub(super) async fn handle_message_reaction(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: &Weak<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    notice: ReactionNotice,
) {
    // Feature toggle first: disabled means zero platform API calls.
    if config
        .disabled_events
        .iter()
        .any(|e| e == crate::channels::EVENT_REACTION)
    {
        debug!(
            channel = %channel_name,
            msg_id = %notice.message_id,
            "reaction ignored (feature disabled)"
        );
        return;
    }
    let target = match adapter.fetch_reaction_target(&notice.message_id).await {
        Ok(Some(t)) => t,
        // A deleted message earns no feedback — there is nothing to
        // point at anymore.
        Ok(None) => {
            debug!(
                channel = %channel_name,
                msg_id = %notice.message_id,
                "reaction target gone (deleted?), skipped"
            );
            return;
        }
        Err(e) => {
            warn!(
                channel = %channel_name,
                msg_id = %notice.message_id,
                error = %e,
                "reaction target fetch failed, skipped"
            );
            return;
        }
    };
    // Watch state is chat-level (mirrors `gate.rs`'s read): a watched
    // chat mirrors the whole conversation, threads included.
    let watch_on = super::gate::read_watch_on(store, config, &target.chat_id).await;
    // Same access control as chat messages (no reaction-back: adding a
    // 🙏 on someone else's message would attribute the denial to the
    // wrong act). Watch parity: a user-allowlist miss still mirrors —
    // explicit blocks, chat lists and a disabled channel always deny.
    if let Err(e) = config.check_access(&target.chat_id, &notice.operator_open_id) {
        let bypass = watch_on
            && matches!(
                e,
                ChannelError::AccessDenied {
                    reason: AccessDeniedReason::UserNotAllowed,
                    ..
                }
            );
        if !bypass {
            info!(channel = %channel_name, error = %e, "reaction access denied");
            return;
        }
    }
    let Some(kernel) = kernel.upgrade() else {
        return;
    };
    // Watch chats: mirror like any plain message — batched, flushed on
    // the next window, never dropped for an idle observer. The name
    // lookup (one uncached contact GET) happens only here, on the path
    // that actually delivers.
    if watch_on {
        let sender_name = adapter.fetch_user_name(&notice.operator_open_id).await;
        let text = steer_text(
            &notice,
            &target,
            sender_name.as_deref(),
            config.platform.name(),
        );
        let msg = ChannelMessage {
            external_chat_id: target.chat_id.clone(),
            external_user_id: notice.operator_open_id.clone(),
            external_message_id: Some(notice.message_id.clone()),
            is_mention: false,
            raw_text: None,
            content: vec![ContentBlock::Text { text }],
            image_keys: Vec::new(),
            thread_id: target.thread_id.clone(),
            root_id: target.root_id.clone(),
            parent_id: None,
            // The mirror path reads only chat id, content and image
            // keys; the rest of the envelope is inert filler.
            is_group: true,
            create_time: notice.action_time,
            doc_comment: None,
        };
        super::watch::mirror_message(
            channel_name,
            store,
            &kernel,
            &msg,
            config.platform.image_ref_tag(),
        )
        .await;
        info!(
            channel = %channel_name,
            chat_id = %target.chat_id,
            msg_id = %notice.message_id,
            emoji = %notice.emoji_type,
            operator = %notice.operator_open_id,
            "reaction enqueued for watch mirroring"
        );
        return;
    }
    // The first mapping hit decides — a less specific key (the chat)
    // must not steal feedback addressed at an idle thread's session.
    let Some(sid) = resolve_target_session(channel_name, store, &notice, &target).await else {
        debug!(
            channel = %channel_name,
            msg_id = %notice.message_id,
            "reaction matched no session, skipped"
        );
        return;
    };
    if !kernel.is_session_running(&sid) {
        debug!(
            channel = %channel_name,
            session_id = %sid.0,
            msg_id = %notice.message_id,
            "reaction for an idle session, skipped"
        );
        return;
    }
    // Dropped reactions pay nothing but the routing GET — the name
    // lookup waits until delivery is certain.
    let sender_name = adapter.fetch_user_name(&notice.operator_open_id).await;
    let text = steer_text(
        &notice,
        &target,
        sender_name.as_deref(),
        config.platform.name(),
    );
    info!(
        channel = %channel_name,
        session_id = %sid.0,
        msg_id = %notice.message_id,
        emoji = %notice.emoji_type,
        operator = %notice.operator_open_id,
        "reaction steered"
    );
    kernel
        .send_steer(&sid, vec![ContentBlock::Text { text }])
        .await;
}

/// The session the reaction points at: the first session-mapping hit
/// among the candidate keys (most specific first). `None` when the
/// reaction landed in a conversation the bot has no session for.
async fn resolve_target_session(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    notice: &ReactionNotice,
    target: &ReactionTarget,
) -> Option<SessionId> {
    for key in candidate_keys(notice, target) {
        match store.find_mapping(channel_name, &key).await {
            Ok(Some(sid)) => return Some(sid),
            Ok(None) => {}
            Err(e) => {
                warn!(channel = %channel_name, error = %e, "reaction mapping lookup failed");
                return None;
            }
        }
    }
    None
}

/// Session-mapping candidate keys, most specific first (mirrors
/// `session_mapping_key`'s key space):
/// 1. the thread's root message — canonical key of in-thread sessions;
/// 2. the thread id itself — legacy in-thread key (pre-root-id events);
/// 3. the reacted message itself — top-level triggers key by their own
///    id under `reply_in_thread` (the bot's answer opens the thread
///    under it), so a reaction on the trigger lands here;
/// 4. the chat — DMs and non-thread groups key by chat id.
fn candidate_keys(notice: &ReactionNotice, target: &ReactionTarget) -> Vec<String> {
    let mut keys: Vec<String> = Vec::with_capacity(4);
    let mut push = |key: &str| {
        if !key.is_empty() && !keys.iter().any(|k| k == key) {
            keys.push(key.to_string());
        }
    };
    if let Some(root) = &target.root_id {
        push(root);
    }
    if let Some(thread) = &target.thread_id {
        push(thread);
    }
    push(&notice.message_id);
    push(&target.chat_id);
    keys
}

/// The steered feedback text: standard metadata envelope (operator as
/// sender, the REACTED message's ids as anchors — `[msg_id: …]` is what
/// earned the reaction, fetchable via the lark skill) over a
/// `[reaction: …]` body carrying the emoji and the target's excerpt.
fn steer_text(
    notice: &ReactionNotice,
    target: &ReactionTarget,
    sender_name: Option<&str>,
    platform: &str,
) -> String {
    let ts = notice
        .action_time
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(chrono::Local::now, |dt| dt.with_timezone(&chrono::Local))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let header = metadata_header(
        &ts,
        HeaderSender::User {
            name: sender_name,
            id: &notice.operator_open_id,
        },
        &target.chat_id,
        &notice.message_id,
        target.thread_id.as_deref(),
        target.root_id.as_deref(),
        platform,
    );
    let body = if target.excerpt.is_empty() {
        format!("[reaction: {}]", notice.emoji_type)
    } else {
        format!("[reaction: {}] {}", notice.emoji_type, target.excerpt)
    };
    format!("{header}\n{body}")
}

#[cfg(test)]
#[path = "reaction_test.rs"]
mod tests;
