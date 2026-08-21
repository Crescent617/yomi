//! Message → session routing: mention rules, thread rules, mapping keys,
//! session creation, and model-key resolution.

use crate::kernel::{CreateSessionInput, Kernel};
use crate::storage::SessionStore;
use crate::types::{Result, SessionId};

use std::sync::Arc;
use tracing::{info, warn};

use super::{
    ChannelConfig, ChannelMessage, ChannelStore, HistoryContainer, PlatformAdapter, PlatformConfig,
};

/// Why a `/thread` command cannot open a thread off this message (its
/// refusal text), if it can't. Telegram has no threads at all — the
/// message-id-keyed session would be an orphan there; without the
/// message id there is nothing to anchor and key by, and the chat-level
/// session must never be hijacked. Also gates the history-cursor
/// advance: a refused command ran nothing and settles no prior context.
pub(crate) fn thread_refusal(config: &ChannelConfig, msg: &ChannelMessage) -> Option<&'static str> {
    // Already in a thread: the command's promise can't be kept —
    // refuse rather than silently run a plain trigger (or worse, fork
    // a parallel session into the same visible thread).
    if msg.thread_id.is_some() {
        return Some("Already in a thread — just send your message directly.");
    }
    if !matches!(config.platform, PlatformConfig::Feishu { .. }) {
        return Some("This platform does not support threads.");
    }
    if msg.external_message_id.is_none() {
        return Some("Cannot open a thread off this message.");
    }
    None
}

/// The history container for a triggering message: its thread when sent
/// inside one, otherwise the chat itself.
pub(crate) fn history_container(msg: &ChannelMessage) -> HistoryContainer {
    match &msg.thread_id {
        Some(tid) => HistoryContainer::Thread(tid.clone()),
        None => HistoryContainer::Chat(msg.external_chat_id.clone()),
    }
}

/// Where a message's effective mention requirement comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MentionSource {
    ThreadOverride,
    ChatOverride,
    Default,
}

impl std::fmt::Display for MentionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::ThreadOverride => "thread override",
            Self::ChatOverride => "chat override",
            Self::Default => "channel default",
        };
        f.write_str(label)
    }
}

/// Read one container's mention override; a store error falls back to
/// no override (i.e. the configured behavior) with a warning rather than
/// silently flipping the gate.
pub(crate) async fn read_mention_override(
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    container_id: &str,
) -> Option<bool> {
    match store.get_mention_override(channel_name, container_id).await {
        Ok(value) => value,
        Err(e) => {
            warn!(error = %e, "mention override read failed, falling back to default");
            None
        }
    }
}

/// The mention requirement applying to a message: the container's own
/// override (thread when inside one, else the chat), then — for threads —
/// the chat's override, then the channel config. DMs never consult the
/// store (the adapter marks them all as mentions anyway).
pub(crate) async fn resolve_require_mention(
    store: &Arc<dyn ChannelStore>,
    config: &ChannelConfig,
    msg: &ChannelMessage,
) -> (bool, MentionSource) {
    if !msg.is_group {
        return (config.require_mention, MentionSource::Default);
    }
    let container = history_container(msg);
    let own = read_mention_override(store, &config.name, container.id()).await;
    if let Some(value) = own {
        let source = if matches!(container, HistoryContainer::Thread(_)) {
            MentionSource::ThreadOverride
        } else {
            MentionSource::ChatOverride
        };
        return (value, source);
    }
    if matches!(container, HistoryContainer::Thread(_)) {
        let chat = read_mention_override(store, &config.name, &msg.external_chat_id).await;
        if let Some(value) = chat {
            return (value, MentionSource::ChatOverride);
        }
    }
    (config.require_mention, MentionSource::Default)
}

/// The effective `reply_in_thread` for a chat: the per-chat override
/// (`/threads on|off`) wins over the channel config. DMs have no
/// override (the command refuses them), so this stays a single lookup.
pub(crate) async fn resolve_reply_in_thread(
    store: &Arc<dyn ChannelStore>,
    config: &ChannelConfig,
    chat_id: &str,
) -> bool {
    match store.get_rit_override(&config.name, chat_id).await {
        Ok(value) => value.unwrap_or(config.reply_in_thread),
        Err(e) => {
            warn!(error = %e, "rit override read failed, falling back to channel config");
            config.reply_in_thread
        }
    }
}

/// The jump link for one `/sessions` entry and whether it points into a
/// thread: anchored at the session's latest routed message (in-thread
/// when the session lives in a thread), falling back to the root message
/// the session keys by, then to a plain chat link. `None` for doc-comment
/// sessions (no chat target) or unsupported platforms.
pub(crate) async fn session_jump_link(
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    sid: &SessionId,
) -> Option<(String, bool)> {
    let routing = store.find_routing_by_session(sid).await.ok()??;
    if routing.doc_comment.is_some() {
        return None;
    }
    if let Some(target) = routing.reply_msg_id.as_deref().or_else(|| {
        routing
            .mapping_key
            .starts_with("om_")
            .then_some(routing.mapping_key.as_str())
    }) {
        if let Some(link) = adapter.thread_link(&routing.external_chat_id, target).await {
            return Some((link, true));
        }
    }
    adapter
        .chat_link(&routing.external_chat_id)
        .await
        .map(|link| (link, false))
}

/// Compute the message ID a reply should be anchored to.
///
/// Replies to in-thread messages always stay in that thread. When the
/// channel's `reply_in_thread` is enabled, group messages additionally anchor
/// to the triggering message so the reply opens/continues its thread
/// (Feishu thread reply, Telegram quote-reply). Private chats are never
/// anchored — threading there is just noise.
pub(crate) fn reply_anchor(msg: &ChannelMessage, reply_in_thread: bool) -> Option<String> {
    msg.external_message_id
        .clone()
        .filter(|_| msg.thread_id.is_some() || (reply_in_thread && msg.is_group))
}

/// Compute the session mapping key for an incoming message.
///
/// In `reply_in_thread` group chats each conversation thread gets its own
/// session. The bot's reply is what opens the thread, so the thread's
/// *starting* message itself carries no `thread_id` — but every message
/// inside the thread carries one and replies to the thread's root message
/// (Feishu sets `root_id` to it). Keying in-thread messages by root id and
/// everything else by its own message id therefore keeps a whole thread in
/// one session while each new top-level message starts a fresh session.
///
/// A plain quote-reply (not in any thread) also carries `root_id` — it must
/// NOT join the quoted message's session: it starts its own, and the bot's
/// `reply_in_thread` answer opens a fresh thread anchored at it.
pub(crate) fn session_mapping_key(
    msg: &ChannelMessage,
    chat_id: &str,
    reply_in_thread: bool,
) -> String {
    // Doc-comment sessions key by the comment thread — one session per
    // comment group, except whole-document comments which carry the
    // WHOLE_COMMENT_ID sentinel (set in comment.rs) and thereby share
    // one session per document — regardless of chat-oriented rules
    // (there is no chat).
    if let Some(dc) = &msg.doc_comment {
        return super::doc_comment_mapping_key(&dc.file_type, &dc.file_token, &dc.comment_id);
    }
    if reply_in_thread && msg.is_group {
        if msg.thread_id.is_some() {
            // Inside a thread: key by the thread's root message so the whole
            // thread shares one session (fall back to thread_id for older
            // event shapes without root_id).
            msg.root_id
                .clone()
                .or_else(|| msg.thread_id.clone())
                .unwrap_or_else(|| chat_id.to_string())
        } else {
            // Top-level trigger or plain quote-reply: a fresh session keyed
            // by the message itself.
            msg.external_message_id
                .clone()
                .unwrap_or_else(|| chat_id.to_string())
        }
    } else {
        msg.thread_id.clone().unwrap_or_else(|| chat_id.to_string())
    }
}

/// The canonical session mapping key: a message inside a thread keys by
/// the thread's ROOT message, one conversation per thread — regardless
/// of chat type (group or private), of `reply_in_thread` mode, and of
/// how the thread started (`/thread` one-shots key by the command
/// message, which IS the future root, so follow-ups resolve to the same
/// key with no special-casing).
///
/// Events don't always carry `root_id` (private chats, older shapes);
/// the root is then resolved from the platform (cached by the adapter —
/// see [`PlatformAdapter::thread_root_id`]). Resolution failure falls
/// back to the plain key (a fresh thread-id-keyed session, the pre-fix
/// behavior) — never an error.
pub(crate) async fn effective_mapping_key(
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    channel_name: &str,
    msg: &ChannelMessage,
    chat_id: &str,
    reply_in_thread: bool,
) -> Result<String> {
    let key = session_mapping_key(msg, chat_id, reply_in_thread);
    if msg.thread_id.is_none() {
        return Ok(key);
    }
    // Fast path: this thread already maps to a session under the
    // computed key.
    if store.find_mapping(channel_name, &key).await?.is_some() {
        return Ok(key);
    }
    // Canonical key: the thread's root message.
    let root_id = match msg.root_id.as_deref() {
        Some(r) => Some(r.to_string()),
        None => adapter
            .thread_root_id(msg.thread_id.as_deref().unwrap_or_default())
            .await
            .unwrap_or_else(|e| {
                warn!(error = %e, "thread root lookup failed, starting a fresh session");
                None
            }),
    };
    let Some(root_id) = root_id.filter(|r| *r != key) else {
        return Ok(key);
    };
    if store.find_mapping(channel_name, &root_id).await?.is_some() {
        info!(
            channel = %channel_name,
            thread_id = msg.thread_id.as_deref().unwrap_or(""),
            root_id,
            "thread resolves to the root message's session"
        );
        return Ok(root_id);
    }
    Ok(key)
}

/// Whether a message is a top-level group message in `reply_in_thread`
/// mode (i.e. not inside any thread). Such messages address the chat as a
/// whole — e.g. a top-level `/model` switches every thread session, and a
/// top-level `/info` shows the chat-level session.
pub(crate) fn is_chat_level_message(msg: &ChannelMessage, reply_in_thread: bool) -> bool {
    reply_in_thread && msg.is_group && msg.thread_id.is_none() && msg.root_id.is_none()
}

/// The subscription scope key for a `/subscribe`/`/unsubscribe` command:
/// the chat id at chat level (never the per-message `reply_in_thread`
/// key — subscriptions bind the conversation, not the command message's
/// own session), the thread's mapping key inside a thread.
pub(crate) async fn subscription_scope_key(
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    channel_name: &str,
    msg: &ChannelMessage,
    chat_id: &str,
    reply_in_thread: bool,
) -> Result<String> {
    if msg.thread_id.is_some() {
        effective_mapping_key(store, adapter, channel_name, msg, chat_id, reply_in_thread).await
    } else {
        Ok(chat_id.to_string())
    }
}

/// Get an existing session or create a new one, updating routing info.
/// The bool reports whether an existing mapping was reused — context-
/// injecting callers read it as "the thread's root is already consumed"
/// (thread mappings are conversation-only, see [`prepare_trigger`]).
pub(crate) async fn get_or_create_session(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    chat_id: &str,
    mapping_key: &str,
    reply_msg_id: Option<&str>,
) -> Result<(SessionId, bool)> {
    // check-then-act（find→create→save）的全局键锁：dispatch 循环是串行
    // 的，但卡片回调（cfg_model 等，spawned 任务）与 ChannelNewThread RPC
    // 都循环外并发可达——同 key 并发 miss 会各建各的 session，败者成
    // 孤儿（消息静默路由到脱离映射的 session）。锁与 ext_route 同键空间。
    let _guard =
        crate::utils::g_lock::g_lock(format!("channel_route:{channel_name}:{mapping_key}")).await;
    if let Some(sid) = store.find_mapping(channel_name, mapping_key).await? {
        info!(channel = %channel_name, mapping_key, session_id = %sid.0, "reusing session");
        store
            .save_mapping(channel_name, mapping_key, &sid, chat_id, reply_msg_id)
            .await?;
        return Ok((sid, true));
    }

    let model_key = model_key_for_new_channel_session(
        channel_name,
        chat_id,
        mapping_key,
        store,
        &kernel.session_store().await,
    )
    .await?;
    let sid = kernel
        .create_session(CreateSessionInput {
            project_id: None,
            working_dir: None,
            auto_approve_level: Some(crate::permission::Level::Dangerous),
            // NB: ask_user 已整体下线（tools/mod.rs 不注册）；conductor
            // 的 blocklist heuristic 随之惰性（历史上按"路由 channel 能否
            // 渲染问题卡"判定）。
            // (CreateSessionInput::tool_blocklist is a reserved field,
            // currently unused).
            tool_blocklist: vec![],
            model_key,
        })
        .await?;
    store
        .save_mapping(channel_name, mapping_key, &sid, chat_id, reply_msg_id)
        .await?;
    info!(channel = %channel_name, mapping_key, session_id = %sid.0, "created session");
    Ok((sid, false))
}

/// The session's model key, or what a fresh session would resolve to
/// (threads inherit the chat session's explicit choice). Read-only.
pub(crate) async fn session_model_key(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    chat_id: &str,
    mapping_key: &str,
) -> Result<String> {
    if let Some(sid) = store.find_mapping(channel_name, mapping_key).await? {
        return Ok(kernel.get_session_model(&sid).await);
    }
    Ok(model_key_for_new_channel_session(
        channel_name,
        chat_id,
        mapping_key,
        store,
        &kernel.session_store().await,
    )
    .await?
    .unwrap_or_else(|| kernel.default_model_key()))
}

/// Resolve the persisted model key for a newly-created channel session.
/// Thread sessions inherit an explicit model choice from their parent chat
/// session. Missing mappings, sessions, or model keys intentionally yield
/// `None`, allowing runtime model resolution to use the configured default
/// without persisting it.
pub(crate) async fn model_key_for_new_channel_session(
    channel_name: &str,
    chat_id: &str,
    mapping_key: &str,
    channel_store: &Arc<dyn ChannelStore>,
    session_store: &Arc<dyn SessionStore>,
) -> Result<Option<String>> {
    if mapping_key == chat_id {
        return Ok(None);
    }

    let Some(parent_session_id) = channel_store.find_mapping(channel_name, chat_id).await? else {
        return Ok(None);
    };

    Ok(session_store
        .get(&parent_session_id)
        .await?
        .and_then(|session| session.model_key))
}
