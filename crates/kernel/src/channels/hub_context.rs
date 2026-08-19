//! Trigger context assembly: history/quote/image prefixes, history cursor,
//! and mid-run receipt bookkeeping.

use crate::kernel::Kernel;

use crate::types::{ContentBlock, Result, SessionId};

use std::sync::Arc;
use tracing::warn;

use super::hub_command::{
    consumes_history, is_command_text, parse_channel_command, ChannelCommand,
};
use super::hub_routing::{
    effective_mapping_key, get_or_create_session, history_container, reply_anchor,
    resolve_reply_in_thread, session_mapping_key, thread_refusal,
};

use super::{
    obs::ObsTracker, ChannelConfig, ChannelMessage, ChannelStore, HistoryContainer, HistoryMessage,
    PlatformAdapter,
};

/// Advance the container's history cursor to a processed message's
/// timestamp (group only, monotonic) — only for messages that settle
/// prior context: run triggers consume it, `/clear` discards it.
pub(crate) async fn advance_history_cursor(
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    msg: &ChannelMessage,
) {
    if config.history_context == 0 || !msg.is_group {
        return;
    }
    let cmd = parse_channel_command(msg.raw_text.as_deref());
    // A refused `/thread` ran nothing — it settles no prior context.
    if !consumes_history(&cmd)
        || (matches!(cmd, ChannelCommand::Thread(_)) && thread_refusal(config, msg).is_some())
    {
        return;
    }
    let Some(ts) = msg.create_time else {
        return;
    };
    let container = history_container(msg);
    let current = store
        .get_history_cursor(channel_name, container.id())
        .await
        .ok()
        .flatten();
    if current.is_none_or(|c| ts > c) {
        let _ = store
            .set_history_cursor(channel_name, container.id(), ts)
            .await;
    }
}

/// Delivery state of a thread's root message for the current trigger.
/// The root of a human-created thread is exactly the context the bot
/// needs — but it must arrive exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootDelivery {
    /// Fresh session: the root has not been delivered yet.
    Pending,
    /// Reused session: an earlier trigger already delivered the root.
    Consumed,
    /// The quoted block (any link of its chain) just delivered the root
    /// for this trigger.
    ByQuote,
}

/// Assemble recent-chat history since the last trigger as a
/// `<recent_chat_history>` block plus images; best-effort. The cursor
/// advances at fetch time, so a later send failure can leave a small
/// gap (accepted). Channel commands in the page are dropped (control-
/// plane — see [`is_command_text`]; the thread root is exempt).
/// `root`: the thread root's delivery state — non-`Pending` dedups
/// it; a `Pending` root missing from the page gets a direct-fetch
/// backstop.
pub(crate) async fn maybe_history_prefix(
    adapter: &Arc<dyn PlatformAdapter>,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    msg: &ChannelMessage,
    root: RootDelivery,
) -> Option<Vec<ContentBlock>> {
    if config.history_context == 0 || !msg.is_group {
        return None;
    }
    // With reply_in_thread, a channel-level trigger opens a fresh thread —
    // the chat's cross-topic chatter is noise there, not context. Triggers
    // inside an existing thread still get that thread's history.
    if msg.thread_id.is_none()
        && resolve_reply_in_thread(store, config, &msg.external_chat_id).await
    {
        return None;
    }
    let container = history_container(msg);
    let cursor = store
        .get_history_cursor(channel_name, container.id())
        .await
        .ok()
        .flatten();
    let messages = match adapter
        .fetch_history(&container, cursor, config.history_context)
        .await
    {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, "history fetch failed, continuing without context");
            return None;
        }
    };
    // Advance the cursor to the newest fetched message so it isn't
    // re-fetched next time.
    if let Some(newest_ts) = messages.iter().map(|m| m.create_time).max() {
        if let Err(e) = store
            .set_history_cursor(channel_name, container.id(), newest_ts)
            .await
        {
            warn!(error = %e, "failed to advance history cursor");
        }
    }
    // Drop the triggering message itself — it's delivered verbatim below.
    let trigger_id = msg.external_message_id.as_deref();
    // The thread root stays unless already delivered (see RootDelivery).
    let drop_root = matches!(&container, HistoryContainer::Thread(_))
        && msg.root_id.is_some()
        && root != RootDelivery::Pending;
    let fetched_root = fetch_root_backstop(adapter, &container, msg, root, &messages, cursor).await;
    let history: Vec<&HistoryMessage> = fetched_root
        .iter()
        .chain(
            messages
                .iter()
                .filter(|m| Some(m.message_id.as_str()) != trigger_id)
                .filter(|m| !drop_root || msg.root_id.as_deref() != Some(m.message_id.as_str()))
                // Commands are control-plane: their replies bypass
                // sessions, so a command line here would show an
                // exchange the bot cannot see. The root is exempt: it
                // has exactly-once delivery semantics, and history[0]
                // must stay the root for the image ordering below.
                .filter(|m| !is_command_text(&m.text)),
        )
        .collect();
    if history.is_empty() {
        return None;
    }
    let quotes = resolve_history_quotes(adapter, &messages, &history).await;
    let mut blocks = vec![ContentBlock::Text {
        text: assemble_history(&history, &quotes),
    }];

    // Attach images behind `[image]`/`[post]` placeholders, capped at
    // the newest few; the backstopped root's images go last to survive.
    let mut pairs = image_pairs(
        history[usize::from(fetched_root.is_some())..]
            .iter()
            .copied(),
    );
    pairs.extend(image_pairs(
        history[..usize::from(fetched_root.is_some())]
            .iter()
            .copied(),
    ));
    if pairs.len() > IMAGE_DOWNLOAD_MAX {
        pairs.drain(..pairs.len() - IMAGE_DOWNLOAD_MAX);
    }
    blocks.extend(download_image_pairs(adapter, &pairs).await);
    Some(blocks)
}

/// Collect (`message_id`, `image_key`) pairs from history messages.
pub(crate) fn image_pairs<'m>(
    messages: impl Iterator<Item = &'m HistoryMessage>,
) -> Vec<(&'m str, &'m str)> {
    messages
        .flat_map(|m| {
            m.image_keys
                .iter()
                .map(move |k| (m.message_id.as_str(), k.as_str()))
        })
        .collect()
}

/// Download (`message_id`, `image_key`) pairs; failures are dropped (the
/// `[image]` text marker already records their presence).
pub(crate) async fn download_image_pairs(
    adapter: &Arc<dyn PlatformAdapter>,
    pairs: &[(&str, &str)],
) -> Vec<ContentBlock> {
    futures::future::join_all(
        pairs
            .iter()
            .map(|(message_id, key)| adapter.download_message_image(message_id, key)),
    )
    .await
    .into_iter()
    .flatten()
    .collect()
}

/// Backstop a still-`Pending` thread root missing from the fetched page
/// with a direct fetch (as the oldest history line); skipped when the
/// cursor covers it (a `/clear` deliberately forgot it).
pub(crate) async fn fetch_root_backstop(
    adapter: &Arc<dyn PlatformAdapter>,
    container: &HistoryContainer,
    msg: &ChannelMessage,
    root: RootDelivery,
    page: &[HistoryMessage],
    cursor: Option<i64>,
) -> Option<HistoryMessage> {
    if root != RootDelivery::Pending {
        return None;
    }
    let (HistoryContainer::Thread(_), Some(root_id)) = (container, &msg.root_id) else {
        return None;
    };
    if page.iter().any(|m| &m.message_id == root_id) {
        return None;
    }
    match adapter.fetch_message(root_id).await {
        Ok(Some(m)) if cursor.is_none_or(|c| m.create_time > c) => Some(m),
        Ok(_) => None,
        Err(e) => {
            warn!(error = %e, root_id, "root backstop fetch failed");
            None
        }
    }
}

/// Max images downloaded per triggering message or injected history
/// block, so an image dump can't blow up the context (or stall delivery
/// behind dozens of downloads). History keeps the newest; a message
/// keeps the first ones and gets a note for the rest.
pub(crate) const IMAGE_DOWNLOAD_MAX: usize = 5;

/// How a run trigger picks its session key and reply anchor.
pub(crate) enum TriggerKind {
    /// The channel's `reply_in_thread` rules (see [`session_mapping_key`]
    /// and [`reply_anchor`]), plus `/thread` adoption.
    Normal,
    /// `/thread`: key by and anchor to the trigger's own message id
    /// regardless of the group-scoped `reply_in_thread` rules, so the
    /// reply opens a thread in any chat. The command arm refuses
    /// triggers without a message id, so the key fallback here is
    /// defensive only.
    OneShotThread,
}

/// Resolve a run trigger's session and assemble its context blocks.
/// `root_in_session` = the mapping predates this trigger — mappings are
/// conversation-only (model commands degrade or fall back to the chat
/// session), so it means the thread's root is already in the session.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_trigger(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    adapter: &Arc<dyn PlatformAdapter>,
    obs: &Arc<ObsTracker>,
    msg: &ChannelMessage,
    kind: TriggerKind,
) -> Result<(SessionId, Vec<ContentBlock>)> {
    let chat_id = msg.external_chat_id.clone();
    let (mapping_key, reply_msg_id) = match kind {
        TriggerKind::OneShotThread => {
            let id = msg.external_message_id.clone();
            (id.clone().unwrap_or_else(|| chat_id.clone()), id)
        }
        TriggerKind::Normal => {
            let rit = resolve_reply_in_thread(store, config, &msg.external_chat_id).await;
            (
                effective_mapping_key(store, adapter, channel_name, msg, &chat_id, rit).await?,
                reply_anchor(msg, rit),
            )
        }
    };
    let (sid, root_in_session) = get_or_create_session(
        channel_name,
        store,
        kernel,
        &chat_id,
        &mapping_key,
        reply_msg_id.as_deref(),
        adapter.supports_status_card(),
    )
    .await?;
    record_receipt(config, obs, kernel, &sid, msg);
    let blocks = context_prefix(adapter, config, store, channel_name, msg, root_in_session).await;
    Ok((sid, blocks))
}

/// Assemble a trigger's context: recent-chat history first, then the
/// quoted message adjacent to the trigger (both best-effort).
/// `root_in_session` seeds the root's delivery state so a human-created
/// thread's root arrives exactly once — quoted first, history as
/// fallback (hence the quoted fetch runs before its block comes last).
pub(crate) async fn context_prefix(
    adapter: &Arc<dyn PlatformAdapter>,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    msg: &ChannelMessage,
    root_in_session: bool,
) -> Vec<ContentBlock> {
    let mut root = if root_in_session {
        RootDelivery::Consumed
    } else {
        RootDelivery::Pending
    };
    let quoted = maybe_quoted_prefix(adapter, msg, root).await;
    // A quoted chain containing the thread root flips the state.
    if quoted.as_ref().is_some_and(|(_, in_chain)| *in_chain) {
        root = RootDelivery::ByQuote;
    }
    // History first, quoted last — the quote belongs to the trigger.
    let mut blocks = maybe_history_prefix(adapter, config, store, channel_name, msg, root)
        .await
        .unwrap_or_default();
    blocks.extend(quoted.map(|(b, _)| b).unwrap_or_default());
    blocks
}

/// Assemble the quoted message as a `<quoted_message>` block plus
/// images, walking its own quote chain (cap [`QUOTE_CHAIN_MAX`]) so a
/// root that is itself a quote-reply keeps its context. In-thread
/// replies to an already-delivered root are skipped; on a fresh
/// (human-created) thread the root is injected like any other quote.
/// The bool reports whether any chain link IS the thread root (drives
/// history dedup). Best-effort: failures degrade to no/partial block.
pub(crate) async fn maybe_quoted_prefix(
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    root: RootDelivery,
) -> Option<(Vec<ContentBlock>, bool)> {
    let parent_id = msg.parent_id.as_deref()?;
    if root != RootDelivery::Pending
        && msg.thread_id.is_some()
        && msg.root_id.as_deref() == Some(parent_id)
    {
        return None;
    }
    // Walk the quote chain: the quoted message first, then its own
    // quoted ancestors.
    let mut chain = Vec::new();
    let mut root_in_chain = false;
    let mut next = Some(parent_id.to_string());
    while let Some(id) = next.take() {
        if chain.len() >= QUOTE_CHAIN_MAX {
            break;
        }
        match adapter.fetch_message(&id).await {
            Ok(Some(m)) => {
                next.clone_from(&m.parent_id);
                root_in_chain |= msg.root_id.as_deref() == Some(m.message_id.as_str());
                chain.push(m);
            }
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, message_id = %id, "quoted message fetch failed");
                break;
            }
        }
    }
    if chain.is_empty() {
        return None;
    }
    // Ancestors first, the quoted message last — chronological reading.
    chain.reverse();
    let lines = chain.iter().map(sender_line).collect::<Vec<_>>().join("\n");
    let mut blocks = vec![ContentBlock::Text {
        text: format!("<quoted_message>\n{lines}\n</quoted_message>"),
    }];
    // The quoted message's own images win over its ancestors' under the
    // cap (chain is oldest-first; iterate newest-first here).
    let pairs = image_pairs(chain.iter().rev());
    blocks
        .extend(download_image_pairs(adapter, &pairs[..pairs.len().min(IMAGE_DOWNLOAD_MAX)]).await);
    Some((blocks, root_in_chain))
}

/// Max messages fetched for one quoted block: the quoted message plus
/// its quote ancestors (bounds latency on pathologically long chains).
pub(crate) const QUOTE_CHAIN_MAX: usize = 3;

/// `[HH:MM] sender: text` (local time, per-message capped) — the shared
/// line format for quoted and history context blocks.
pub(crate) fn sender_line(m: &HistoryMessage) -> String {
    let ts = chrono::DateTime::from_timestamp_millis(m.create_time)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default();
    let text = crate::utils::strs::truncate_by_chars(m.text.trim(), HISTORY_MESSAGE_MAX_CHARS, "…");
    format!("[{ts}] {}: {text}", m.sender_id)
}

/// Download a message's attached images (deferred until post-gate) and
/// append them to the content, capped at [`IMAGE_DOWNLOAD_MAX`]; a
/// failure degrades to a visible text placeholder instead of dropping
/// the message.
pub(crate) async fn append_message_images(
    adapter: &Arc<dyn PlatformAdapter>,
    message_id: &str,
    image_keys: &[String],
    content: &mut Vec<ContentBlock>,
) {
    let omitted = image_keys.len().saturating_sub(IMAGE_DOWNLOAD_MAX);
    let keys = &image_keys[..image_keys.len() - omitted];
    let results = futures::future::join_all(
        keys.iter()
            .map(|key| adapter.download_message_image(message_id, key)),
    )
    .await;
    for (key, result) in keys.iter().zip(results) {
        match result {
            Ok(block) => content.push(block),
            Err(e) => {
                warn!(error = %e, image_key = %key, "image download failed");
                content.push(ContentBlock::Text {
                    text: format!("[Failed to download image: {e}]"),
                });
            }
        }
    }
    if omitted > 0 {
        content.push(ContentBlock::Text {
            text: format!("[{omitted} more image(s) omitted]"),
        });
    }
}

/// Per-message cap in the injected history block (UTF-8 safe truncation).
pub(crate) const HISTORY_MESSAGE_MAX_CHARS: usize = 2000;

/// Format fetched messages as a context block: chronological, one line
/// each (`[HH:MM] open_id: text`, per-message capped), quote-replies
/// carrying an inline snippet of the quoted message (` ↩ sender: text`).
pub(crate) fn assemble_history(
    messages: &[&HistoryMessage],
    quotes: &std::collections::HashMap<String, String>,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("<recent_chat_history>\n");
    for m in messages {
        let _ = write!(out, "{}", sender_line(m));
        if let Some(q) = quotes.get(&m.message_id) {
            let _ = write!(out, " ↩ {q}");
        }
        let _ = writeln!(out);
    }
    out.push_str("</recent_chat_history>");
    out
}

/// Max quoted parents fetched per history block for inline snippets
/// (bounds extra latency; parents already in the fetched page are free).
pub(crate) const HISTORY_QUOTE_FETCH_MAX: usize = 3;

/// Per-quote snippet cap in history lines (quotes are secondary context).
pub(crate) const QUOTE_SNIPPET_MAX_CHARS: usize = 80;

/// `sender: text` for an inline quote snippet (whitespace-collapsed to
/// keep the one-line-per-message block shape).
pub(crate) fn quote_snippet(m: &HistoryMessage) -> String {
    let collapsed = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = crate::utils::strs::truncate_by_chars(&collapsed, QUOTE_SNIPPET_MAX_CHARS, "…");
    format!("{}: {text}", m.sender_id)
}

/// Resolve quoted-message snippets for quote-replies in `history` (one
/// level only — history is background context). Parents already in the
/// fetched page are free; others are fetched directly, distinct parents
/// capped at [`HISTORY_QUOTE_FETCH_MAX`]. Keyed by history message id.
pub(crate) async fn resolve_history_quotes(
    adapter: &Arc<dyn PlatformAdapter>,
    page: &[HistoryMessage],
    history: &[&HistoryMessage],
) -> std::collections::HashMap<String, String> {
    let mut quotes = std::collections::HashMap::new();
    let mut fetched: std::collections::HashMap<&str, Option<String>> =
        std::collections::HashMap::new();
    for m in history {
        let Some(parent_id) = m.parent_id.as_deref() else {
            continue;
        };
        // Free lookups: the kept history (incl. a backstopped root) and
        // the fetched page (incl. a dropped root).
        let parent = history
            .iter()
            .find(|p| p.message_id == parent_id)
            .copied()
            .or_else(|| page.iter().find(|p| p.message_id == parent_id));
        if let Some(p) = parent {
            quotes.insert(m.message_id.clone(), quote_snippet(p));
            continue;
        }
        if fetched.len() >= HISTORY_QUOTE_FETCH_MAX && !fetched.contains_key(parent_id) {
            continue;
        }
        let snippet = match fetched.entry(parent_id) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let snippet = match adapter.fetch_message(parent_id).await {
                    Ok(Some(p)) => Some(quote_snippet(&p)),
                    Ok(None) => None,
                    Err(err) => {
                        warn!(error = %err, parent_id, "history quote fetch failed");
                        None
                    }
                };
                e.insert(snippet)
            }
        };
        if let Some(snippet) = snippet {
            quotes.insert(m.message_id.clone(), snippet.clone());
        }
    }
    quotes
}

/// Record a user message posted while the session's agent is running, for
/// the mid-run post detection (morph vs. new-message settle). Messages that
/// arrive while the session is idle are run triggers, not mid-run posts —
/// recording only while running also means a receipt can never outlive its
/// run into the next one (settlement clears them). Every accepted message
/// additionally refreshes the session's settle-reaction target (its latest
/// user message), so async runs without a fresh trigger still have one.
/// No-op when observability is disabled or the message carries no platform ID.
pub(crate) fn record_receipt(
    config: &ChannelConfig,
    obs: &ObsTracker,
    kernel: &Kernel,
    session_id: &SessionId,
    msg: &ChannelMessage,
) {
    if !config.observability {
        return;
    }
    if let Some(msg_id) = &msg.external_message_id {
        obs.record_user_msg(session_id, msg_id.clone());
    }
    if !kernel.is_session_running(session_id) {
        return;
    }
    if let Some(msg_id) = &msg.external_message_id {
        obs.record_receipt(session_id, msg_id.clone());
    }
}

/// Record a mid-run receipt for a message NOT addressed to the bot
/// (mention-missed group chatter). The message itself stays unprocessed
/// — no session is created, no reply, no reaction — but if it belongs to
/// a session whose agent is running, it counts as a mid-run post: the
/// user is still talking in that conversation, so the run's reply should
/// sink below their message instead of morphing above it. The
/// settle-reaction target is untouched (the bot never engaged with the
/// message), and commands are skipped — consistent with addressed
/// receipts, which record only on the Steer/Queue/plain-text routes.
pub(crate) async fn record_passive_receipt(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    obs: &ObsTracker,
    msg: &ChannelMessage,
    is_running: impl Fn(&SessionId) -> bool,
) {
    if !config.observability {
        return;
    }
    let Some(msg_id) = msg.external_message_id.as_deref() else {
        return;
    };
    if !matches!(
        parse_channel_command(msg.raw_text.as_deref()),
        ChannelCommand::None
    ) {
        return;
    }
    let rit = resolve_reply_in_thread(store, config, &msg.external_chat_id).await;
    // Passive path: never resolve roots via the platform (that would
    // cost an API lookup for chatter the bot wasn't even addressed in).
    // The plain key resolves rit=on threads (key == root); for other
    // shapes a present `root_id` is a free database-only fallback.
    let mapping_key = session_mapping_key(&msg, &msg.external_chat_id, rit);
    // A top-level group message in reply_in_thread mode keys by its own
    // id — never mapped, never a mid-run post (it doesn't interleave
    // with any thread's run).
    let sid = match store.find_mapping(channel_name, &mapping_key).await {
        Ok(Some(sid)) => sid,
        Ok(None) => {
            let Some(root_id) = msg.root_id.as_deref().filter(|r| *r != mapping_key) else {
                return;
            };
            match store.find_mapping(channel_name, root_id).await {
                Ok(Some(sid)) => sid,
                _ => return,
            }
        }
        Err(_) => return,
    };
    if !is_running(&sid) {
        return;
    }
    obs.record_receipt(&sid, msg_id.to_string());
}
