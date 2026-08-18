//! Slash-command handlers (`handle_incoming_message` and the
//! `/bind` `/mention` `/threads` `/sessions` blocks).

use crate::kernel::Kernel;

use crate::types::{ContentBlock, Result, SessionId};

use std::sync::Arc;
use tracing::warn;

use super::hub_command::{
    format_current_model, format_model_list, format_session_info, format_unknown_model,
    parse_channel_command, ChannelCommand, OverrideMode, HELP_TEXT,
};
use super::hub_context::{append_message_images, prepare_trigger, TriggerKind};
use super::hub_routing::{
    effective_mapping_key, get_or_create_session, history_container, is_chat_level_message,
    reply_anchor, resolve_reply_in_thread, resolve_require_mention, session_jump_link,
    session_model_key, subscription_scope_key, thread_refusal,
};

use super::{
    obs::ObsTracker, ChannelConfig, ChannelMessage, ChannelStore, PlatformAdapter, PlatformConfig,
};

pub(crate) async fn handle_incoming_message(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: Arc<Kernel>,
    msg: ChannelMessage,
    obs: &Arc<ObsTracker>,
    adapter: &Arc<dyn PlatformAdapter>,
) -> Result<Option<String>> {
    let chat_id = msg.external_chat_id.clone();
    let rit = resolve_reply_in_thread(store, config, &chat_id).await;
    let reply_msg_id = reply_anchor(&msg, rit);
    let mapping_key =
        effective_mapping_key(store, adapter, channel_name, &msg, &chat_id, rit).await?;
    tracing::debug!(
        channel = %channel_name,
        chat_id = %chat_id,
        msg_id = msg.external_message_id.as_deref().unwrap_or(""),
        thread_id = msg.thread_id.as_deref().unwrap_or(""),
        root_id = msg.root_id.as_deref().unwrap_or(""),
        mapping_key = %mapping_key,
        "session mapping"
    );

    let cmd = parse_channel_command(msg.raw_text.as_deref());
    match cmd {
        ChannelCommand::Help => Ok(Some(HELP_TEXT.to_string())),
        ChannelCommand::Clear => {
            if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
                if let Err(e) = kernel.clear_session(&sid) {
                    tracing::warn!("Failed to clear session {}: {}", sid.0, e);
                }
            }
            Ok(Some("Context cleared.".to_string()))
        }
        ChannelCommand::Compact => {
            // Fire-and-forget: with observability the compact gets its own
            // status card (materialized on `Compacting`, settled by
            // `Compacted`) — that card is the feedback. Otherwise this
            // text ack is the only one (the outcome is only logged).
            if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
                if let Err(e) = kernel.compact_session(&sid) {
                    // Publish failed — no events will fire, so neither the
                    // card nor a swallowed ack may stand in for feedback.
                    warn!(error = %e, "compact request publish failed");
                    return Ok(Some(
                        "⚠️ Failed to start compaction (busy) — please retry.".to_string(),
                    ));
                }
                let card_covers = config.observability
                    && adapter.supports_status_card()
                    && msg.doc_comment.is_none();
                if card_covers {
                    return Ok(None);
                }
                return Ok(Some("⏳ Compacting context…".to_string()));
            }
            Ok(Some("No session to compact.".to_string()))
        }
        ChannelCommand::Stop => {
            if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
                kernel.cancel(&sid);
                return Ok(Some("Stopped.".to_string()));
            }
            Ok(Some("No active session to stop.".to_string()))
        }
        ChannelCommand::Restart => {
            if let Some(deny) = super::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            if !kernel.can_restart() {
                return Ok(Some("Restart is not supported by this daemon.".to_string()));
            }
            let runs = kernel.live_session_count();
            let text = if runs == 0 {
                "🔄 Restarting daemon…".to_string()
            } else {
                format!("🔄 Restarting daemon… {runs} active run(s) will be interrupted.")
            };
            // The ack is sent inline: the usual spawned command reply
            // could be aborted when the daemon shuts down mid-flight.
            if let Err(e) = adapter
                .send_message(
                    &chat_id,
                    vec![ContentBlock::Text { text }],
                    reply_msg_id.as_deref(),
                )
                .await
            {
                warn!(channel = %channel_name, error = %e, "restart ack send failed");
            }
            kernel.request_restart();
            Ok(None)
        }
        ChannelCommand::Steer(text) => {
            let (sid, mut blocks) = prepare_trigger(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                obs,
                &msg,
                TriggerKind::Normal,
            )
            .await?;
            kernel.note_title_input(&sid, &text);
            blocks.push(ContentBlock::Text { text });
            kernel.send_steer(&sid, blocks).await;
            Ok(None)
        }
        ChannelCommand::Thread(text) => {
            // One-shot thread: key by and anchor to the command
            // message itself, so the reply opens a thread off it (see
            // prepare_trigger); follow-ups inside adopt the session
            // via the thread root (see effective_mapping_key).
            if let Some(refusal) = thread_refusal(config, &msg) {
                return Ok(Some(refusal.to_string()));
            }
            let (sid, mut blocks) = prepare_trigger(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                obs,
                &msg,
                TriggerKind::OneShotThread,
            )
            .await?;
            kernel.note_title_input(&sid, &text);
            blocks.push(ContentBlock::Text { text });
            // Deferred image download — as for a plain trigger, only
            // now, after the gate, does an attached image cost
            // bandwidth.
            append_message_images(
                adapter,
                msg.external_message_id.as_deref().unwrap_or(""),
                &msg.image_keys,
                &mut blocks,
            )
            .await;
            kernel.send_steer(&sid, blocks).await;
            Ok(None)
        }
        ChannelCommand::InvalidThreadCommand => Ok(Some(
            "Usage: `/thread <text>` — the reply opens a new thread.".to_string(),
        )),
        ChannelCommand::InvalidSteerCommand => Ok(Some(
            "Usage: `/steer <text>` — inject a message into the current run.".to_string(),
        )),
        ChannelCommand::InvalidQueueCommand => Ok(Some(
            "Usage: `/queue <text>` — queue a message for a later turn.".to_string(),
        )),
        ChannelCommand::Queue(text) => {
            let (sid, mut blocks) = prepare_trigger(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                obs,
                &msg,
                TriggerKind::Normal,
            )
            .await?;
            kernel.note_title_input(&sid, &text);
            blocks.push(ContentBlock::Text { text });
            // The title was just fed from the user's own text — don't
            // let send_message re-extract it from the merged blocks.
            kernel.send_message_inner(&sid, blocks, false).await?;
            Ok(None)
        }
        ChannelCommand::ListModels => {
            let models = kernel.list_models().await?;
            let current =
                session_model_key(channel_name, store, &kernel, &chat_id, &mapping_key).await?;
            Ok(Some(format_model_list(&models, &current)))
        }
        ChannelCommand::CurrentModel => {
            let models = kernel.list_models().await?;
            let current =
                session_model_key(channel_name, store, &kernel, &chat_id, &mapping_key).await?;
            Ok(Some(format_current_model(&models, &current)))
        }
        ChannelCommand::SwitchModel(key) => {
            let models = kernel.list_models().await?;
            if !models.iter().any(|model| model.name == key) {
                return Ok(Some(format_unknown_model(&key, &models)));
            }
            // Chat level — or a thread without a session: switch the chat
            // session instead and let the thread inherit it (also keeps
            // thread mappings conversation-only).
            let chat_level = is_chat_level_message(&msg, rit)
                || store
                    .find_mapping(channel_name, &mapping_key)
                    .await?
                    .is_none();
            if chat_level {
                // Switch the whole chat: update every existing thread
                // session routed to this chat, and persist the choice on
                // the chat-level session so future threads inherit it.
                let (chat_sid, _) =
                    get_or_create_session(channel_name, store, &kernel, &chat_id, &chat_id, None)
                        .await?;
                kernel.set_session_model(&chat_sid, &key).await?;
                for (mk, sid) in store.list_mappings(channel_name).await? {
                    if mk == chat_id {
                        continue;
                    }
                    if let Ok(Some(routing)) = store.find_routing_by_session(&sid).await {
                        if routing.external_chat_id == chat_id {
                            kernel.set_session_model(&sid, &key).await?;
                        }
                    }
                }
                return Ok(Some(format!(
                    "Switched all threads in this chat to `{key}`. It takes effect on the next model invocation."
                )));
            }
            // find_mapping above proved the mapping exists; this only
            // refreshes its routing anchor.
            let (sid, _) = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
            )
            .await?;
            kernel.set_session_model(&sid, &key).await?;
            Ok(Some(format!(
                "Switched to `{key}`. It takes effect on the next model invocation."
            )))
        }
        ChannelCommand::InvalidModelCommand => Ok(Some(
            "Usage: `/model` or `/model <model_key>`. Use `/models` to list models.".to_string(),
        )),
        ChannelCommand::Mention(mode) => {
            handle_mention_command(config, store, &msg, mode).await.map(Some)
        }
        ChannelCommand::InvalidMentionCommand => Ok(Some(
            "Usage: `/mention` to show the current setting; `/mention on|off|reset` to change it (admin)."
                .to_string(),
        )),
        ChannelCommand::Threads(mode) => {
            handle_threads_command(config, store, &msg, mode).await.map(Some)
        }
        ChannelCommand::InvalidThreadsCommand => Ok(Some(
            "Usage: `/threads` to show the current setting; `/threads on|off|reset` to change it (admin)."
                .to_string(),
        )),
        ChannelCommand::Sessions(offset) => {
            handle_sessions_command(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                &msg,
                reply_msg_id.clone(),
                offset,
            )
            .await
        }
        ChannelCommand::InvalidSessionsCommand => Ok(Some(
            "Usage: `/sessions` for the 10 most recent sessions; `/sessions <offset>` for the next page (admin)."
                .to_string(),
        )),
        ChannelCommand::Info => {
            // Chat-level messages show the chat session, in-thread ones
            // the thread's. Read-only: never creates a session or mapping.
            let chat_level = is_chat_level_message(&msg, rit);
            let key = if chat_level { &chat_id } else { &mapping_key };
            let Some(sid) = store.find_mapping(channel_name, key).await? else {
                let model_key =
                    session_model_key(channel_name, store, &kernel, &chat_id, key).await?;
                return Ok(Some(format!(
                    "No session yet in this {}. First message will use `{model_key}`.",
                    if chat_level { "chat" } else { "thread" },
                )));
            };
            let session = kernel.get_session(&sid).await?;
            let model_key = kernel.get_session_model(&sid).await;
            let models = kernel.list_models().await?;
            let running_subagents = kernel
                .list_subagents(&sid)
                .await?
                .into_iter()
                .filter(|s| s.is_running)
                .count();
            let shells = kernel.list_background_shells(&sid);
            Ok(Some(format_session_info(
                &session,
                &model_key,
                &models,
                running_subagents,
                &shells,
            )))
        }
        ChannelCommand::Permits => {
            super::approval::list_pending(channel_name, config, store, &msg.external_user_id).await
        }
        ChannelCommand::Approve { id, perm } => {
            super::approval::approve(
                channel_name,
                config,
                store,
                adapter,
                &msg.external_user_id,
                id,
                perm.as_deref(),
            )
            .await
        }
        ChannelCommand::Deny { id } => {
            super::approval::deny(
                channel_name,
                config,
                store,
                adapter,
                &msg.external_user_id,
                id,
            )
            .await
        }
        ChannelCommand::InvalidApprovalCommand => Ok(Some(super::approval::usage())),
        ChannelCommand::Subscribe {
            recursive,
            target_chat_id,
        } => {
            // Subscriptions bind a chat conversation scope — meaningless
            // for a doc comment thread (and would write orphan rows).
            if msg.doc_comment.is_some() {
                return Ok(Some(
                    "Subscriptions are not available for doc comment sessions.".to_string(),
                ));
            }
            // Jump-link notifications rely on platform message links (and
            // DM cards for the default target) — Feishu only for now.
            if !matches!(config.platform, PlatformConfig::Feishu { .. }) {
                return Ok(Some(
                    "This platform does not support subscriptions yet.".to_string(),
                ));
            }
            let in_thread = msg.thread_id.is_some();
            if in_thread && recursive {
                return Ok(Some(
                    "Recursive subscription is only meaningful at chat level — a thread subscription already covers exactly this thread. Use `/subscribe -r` outside the thread to cover the whole chat."
                        .to_string(),
                ));
            }
            let scope_key =
                subscription_scope_key(store, adapter, channel_name, &msg, &chat_id, rit)
                    .await?;
            store
                .save_run_subscription(
                    channel_name,
                    &scope_key,
                    &chat_id,
                    recursive,
                    &msg.external_user_id,
                    target_chat_id.as_deref(),
                )
                .await?;
            let scope_text = match (in_thread, recursive) {
                (true, _) => "this thread",
                (false, true) => "this chat (including all its threads)",
                (false, false) => "this chat",
            };
            let target_text = match &target_chat_id {
                Some(t) => format!("; notifications will be posted to `{t}`"),
                None => String::new(),
            };
            // In reply_in_thread group chats every top-level trigger opens
            // its own thread, so a non-recursive chat subscription can
            // never match a run — say so instead of silently no-oping.
            let rit_hint = if !in_thread && !recursive && rit && msg.is_group {
                " Note: this chat replies in threads — every new question starts its own thread, which this subscription does NOT cover; use `/subscribe -r` to get notified here."
            } else {
                ""
            };
            Ok(Some(format!(
                "✅ Subscribed to {scope_text}{target_text} — I'll DM you when runs here complete.{rit_hint} `/unsubscribe` to cancel."
            )))
        }
        ChannelCommand::Unsubscribe => {
            if msg.doc_comment.is_some() {
                return Ok(Some(
                    "Subscriptions are not available for doc comment sessions.".to_string(),
                ));
            }
            let scope_key =
                subscription_scope_key(store, adapter, channel_name, &msg, &chat_id, rit)
                    .await?;
            let removed = store
                .remove_run_subscription(channel_name, &scope_key, &msg.external_user_id)
                .await?;
            Ok(Some(if removed > 0 {
                "Unsubscribed.".to_string()
            } else {
                "You have no subscription here.".to_string()
            }))
        }
        ChannelCommand::InvalidSubscribeCommand => Ok(Some(
            "Usage: `/subscribe [chat_id] [-r|--recursive]` or `/unsubscribe`.".to_string(),
        )),
        ChannelCommand::Bind(target) => {
            handle_bind(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                &msg,
                &chat_id,
                &mapping_key,
                reply_msg_id.clone(),
                target,
            )
            .await
            .map(Some)
        }
        ChannelCommand::InvalidBindCommand => {
            Ok(Some("Usage: `/bind` or `/bind <session_id>`.".to_string()))
        }
        ChannelCommand::Unknown(cmd) => Ok(Some(format!(
            "Unknown command `{cmd}`. See `/help` for the command list."
        ))),
        ChannelCommand::None => {
            let (sid, mut content) = prepare_trigger(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                obs,
                &msg,
                TriggerKind::Normal,
            )
            .await?;
            // Title from the user's bare text: msg.content carries the
            // adapter's metadata header ([ts][from_user_id:…]), and
            // context blocks merge ahead of it (see note_title_input).
            if let Some(raw) = msg.raw_text.as_deref() {
                kernel.note_title_input(&sid, raw);
            }
            content.extend(msg.content);
            // Deferred image download — only now, after the gate, does
            // an attached image cost bandwidth.
            append_message_images(
                adapter,
                msg.external_message_id.as_deref().unwrap_or(""),
                &msg.image_keys,
                &mut content,
            )
            .await;
            kernel.send_steer(&sid, content).await;
            Ok(None)
        }
    }
}

/// `/bind`: show or retarget the current scope's session binding.
/// Retargeting is admin-only. A session already routed elsewhere is
/// refused: for chat scopes that means another chat/channel (a reply
/// could land in the wrong chat); for doc-comment scopes, ANY other
/// mapping (the delivery target comes from the mapping row itself, so
/// sharing across comment threads would post answers to the wrong
/// document). Unrouted sessions (GUI/CLI-created) are free to adopt.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_bind(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    chat_id: &str,
    mapping_key: &str,
    reply_msg_id: Option<String>,
    target: Option<String>,
) -> Result<String> {
    let current = store.find_mapping(channel_name, mapping_key).await?;
    let Some(target) = target else {
        return Ok(match current {
            Some(sid) => format!(
                "Current session: `{}`. Retarget with `/bind <session_id>`.",
                sid.0
            ),
            None => "No session here yet — the first message will create one. \
                     Adopt an existing one with `/bind <session_id>`."
                .to_string(),
        });
    };
    if let Some(deny) = super::approval::check_admin(config, &msg.external_user_id) {
        return Ok(deny);
    }
    let sid = SessionId::from(target.clone());
    let session = match kernel.get_session(&sid).await {
        Ok(s) => s,
        Err(_) => return Ok(format!("Session `{target}` not found.")),
    };
    if current.as_ref() == Some(&sid) {
        return Ok(format!("Already bound to `{target}` here."));
    }
    // Retargeting a routed session is a move, not a share: delivery
    // resolves a single mapping row per session (`find_routing_by_session`),
    // so the old rows must go — otherwise replies keep posting to the
    // previous conversation.
    let mut moved = false;
    if let Some(routing) = store.find_routing_by_session(&sid).await? {
        let compatible = if msg.doc_comment.is_some() {
            routing.mapping_key == mapping_key
        } else {
            routing.channel_name == channel_name && routing.external_chat_id == chat_id
        };
        if !compatible {
            return Ok(format!(
                "`{target}` is bound to another conversation; refusing to rebind."
            ));
        }
        // Say goodbye in the old conversation — its members otherwise just
        // see a bot that suddenly forgot everything (doc-comment scopes
        // have no chat to post to). Fire-and-forget, like the restart ack.
        if routing.doc_comment.is_none() {
            let text = format!(
                "Session `{target}` has moved to another conversation — \
                 the next message here starts fresh."
            );
            if let Err(e) = adapter
                .send_message(
                    &routing.external_chat_id,
                    vec![ContentBlock::Text { text }],
                    routing.reply_msg_id.as_deref(),
                )
                .await
            {
                warn!(channel = %channel_name, error = %e, "bind farewell send failed");
            }
        }
        store.delete_by_sessions(std::slice::from_ref(&sid)).await?;
        moved = true;
    }
    store
        .save_mapping(
            channel_name,
            mapping_key,
            &sid,
            chat_id,
            reply_msg_id.as_deref(),
        )
        .await?;
    let title = session
        .title
        .map(|t| format!(" 「{t}」"))
        .unwrap_or_default();
    // Name the session this retarget displaces, so it can be bound back.
    let previous = current.map_or_else(String::new, |old| {
        format!(
            "\nPreviously bound here: `{}` — bind back with `/bind {}`.",
            old.0, old.0
        )
    });
    if moved {
        Ok(format!(
            "✅ Moved `{target}`{title} here — its previous conversation no longer receives its replies.{previous}"
        ))
    } else {
        Ok(format!(
            "✅ Bound this conversation to `{target}`{title}.{previous}"
        ))
    }
}

/// `/mention` query or mutation. The override lives on the message's
/// container; mutations are admin-only — turning the requirement off
/// makes the bot answer every group message, a cost amplifier.
pub(crate) async fn handle_mention_command(
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    msg: &ChannelMessage,
    mode: Option<OverrideMode>,
) -> Result<String> {
    if !msg.is_group {
        return Ok("No need for this in DMs — every message is answered.".to_string());
    }
    let on_off = |v: bool| if v { "on" } else { "off" };
    let container = history_container(msg);
    let scope = container.label();
    let Some(mode) = mode else {
        let (effective, source) = resolve_require_mention(store, config, msg).await;
        return Ok(format!(
            "Mention requirement in this {scope}: `{}` ({source}); channel default: `{}`.",
            on_off(effective),
            on_off(config.require_mention),
        ));
    };
    if let Some(deny) = super::approval::check_admin(config, &msg.external_user_id) {
        return Ok(deny);
    }
    match mode {
        OverrideMode::On | OverrideMode::Off => {
            let value = matches!(mode, OverrideMode::On);
            store
                .set_mention_override(&config.name, container.id(), value)
                .await?;
            Ok(format!(
                "Mention requirement set to `{}` for this {scope} (channel default: `{}`).",
                on_off(value),
                on_off(config.require_mention),
            ))
        }
        OverrideMode::Reset => {
            store
                .clear_mention_override(&config.name, container.id())
                .await?;
            let (effective, source) = resolve_require_mention(store, config, msg).await;
            Ok(format!(
                "Override cleared for this {scope}; now following {source}: `{}`.",
                on_off(effective),
            ))
        }
    }
}

/// `/threads`: query or mutate the chat-level `reply_in_thread`
/// override. The mode only makes sense for whole chats — a thread
/// exists because of it — so the chat is the only scope (unlike
/// `/mention`, threads carry no own override).
pub(crate) async fn handle_threads_command(
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    msg: &ChannelMessage,
    mode: Option<OverrideMode>,
) -> Result<String> {
    if !msg.is_group {
        return Ok("No need for this in DMs — replies are never threaded.".to_string());
    }
    let on_off = |v: bool| if v { "on" } else { "off" };
    let chat_id = &msg.external_chat_id;
    let Some(mode) = mode else {
        let override_value = store
            .get_rit_override(&config.name, chat_id)
            .await
            .ok()
            .flatten();
        let (effective, source) = match override_value {
            Some(v) => (v, "chat override"),
            None => (config.reply_in_thread, "channel default"),
        };
        return Ok(format!(
            "Reply-in-thread in this chat: `{}` ({source}); channel default: `{}`.",
            on_off(effective),
            on_off(config.reply_in_thread),
        ));
    };
    if let Some(deny) = super::approval::check_admin(config, &msg.external_user_id) {
        return Ok(deny);
    }
    match mode {
        OverrideMode::On | OverrideMode::Off => {
            let value = matches!(mode, OverrideMode::On);
            store.set_rit_override(&config.name, chat_id, value).await?;
            // Existing sessions keep their mapping; the mode only shapes
            // how new messages route.
            let note = if value {
                " New top-level messages will each open their own thread."
            } else {
                " New messages will share the chat-level session; existing threads keep working."
            };
            Ok(format!(
                "Reply-in-thread set to `{}` for this chat (channel default: `{}`).{note}",
                on_off(value),
                on_off(config.reply_in_thread),
            ))
        }
        OverrideMode::Reset => {
            store.clear_rit_override(&config.name, chat_id).await?;
            Ok(format!(
                "Override cleared for this chat; now following the channel default: `{}`.",
                on_off(config.reply_in_thread),
            ))
        }
    }
}

/// Page size for `/sessions` (and the scan chunk when filtering to
/// channel-routed sessions).
pub(crate) const SESSIONS_PAGE_SIZE: usize = 10;

pub(crate) const SESSIONS_SCAN_LIMIT: usize = 50;

/// `/sessions [offset]` (admin): this channel's most recent sessions,
/// each with a click-to-jump link into its thread or chat. Offset pages
/// through the list (`/sessions 20` skips the first 20 matches).
/// Card-capable platforms get a fancy card (reply is `None`); everyone
/// else gets a plain text list.
pub(crate) async fn handle_sessions_command(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    reply_msg_id: Option<String>,
    offset: usize,
) -> Result<Option<String>> {
    if let Some(deny) = super::approval::check_admin(config, &msg.external_user_id) {
        return Ok(Some(deny));
    }
    // Channel-routed sessions only — a jump needs a delivery target.
    let routed: std::collections::HashSet<String> = store
        .list_mappings(channel_name)
        .await?
        .into_iter()
        .map(|(_, sid)| sid.0.to_string())
        .collect();

    // Sessions paginate by cursor (updated_at desc); offset pages over
    // the *routed* matches.
    let mut picked: Vec<crate::storage::session::SessionInfo> = Vec::new();
    let mut skipped = 0usize;
    let mut before = None;
    let mut has_more = false;
    'scan: loop {
        let page = kernel
            .list_sessions(
                None,
                crate::storage::session::SessionListScope::All,
                before,
                SESSIONS_SCAN_LIMIT,
            )
            .await?;
        let page_has_more = page.next_cursor.is_some();
        let Some(last) = page.sessions.last() else {
            break;
        };
        before = Some(last.updated_at);
        for info in page.sessions {
            if info.id.0.starts_with("sub_") || !routed.contains(info.id.0.as_str()) {
                continue;
            }
            if skipped < offset {
                skipped += 1;
                continue;
            }
            if picked.len() < SESSIONS_PAGE_SIZE {
                picked.push(info);
            } else {
                has_more = true;
                break 'scan;
            }
        }
        if !page_has_more {
            break;
        }
    }

    if picked.is_empty() {
        return Ok(Some(if offset == 0 {
            "This channel has no sessions yet.".to_string()
        } else {
            format!("No more sessions beyond offset {offset}.")
        }));
    }

    // One call for the whole page: "active" = running or holding
    // background tasks (the same semantics as `list_running_sessions`).
    let running: std::collections::HashSet<String> = kernel
        .list_running_sessions()
        .await?
        .into_iter()
        .map(|s| s.id.0.to_string())
        .collect();

    let mut entries = Vec::with_capacity(picked.len());
    let now = chrono::Utc::now();
    // Fetch all jump links concurrently — a serial loop would stall the
    // channel's single dispatch loop for one API call per row.
    let links = futures::future::join_all(
        picked
            .iter()
            .map(|info| session_jump_link(store, adapter, &info.id)),
    )
    .await;
    for (info, link) in picked.iter().zip(links) {
        let active = running.contains(info.id.0.as_str());
        let marker = if active {
            "⚡"
        } else if link.as_ref().is_some_and(|(_, is_thread)| *is_thread) {
            "🧵"
        } else {
            "💬"
        };
        entries.push(SessionEntry {
            marker,
            title: session_link_title(info),
            bucket: session_time_bucket(info.updated_at, now),
            link: link.map(|(url, _thread)| url),
        });
    }

    // Card-capable platforms get the fancy card; everyone else gets the
    // plain text list. Doc-comment commands have no chat to card into.
    if msg.doc_comment.is_none() && adapter.supports_status_card() {
        let card = sessions_card(offset, &entries, has_more);
        adapter
            .send_card(&msg.external_chat_id, &card, reply_msg_id.as_deref())
            .await?;
        return Ok(None);
    }
    Ok(Some(sessions_text(offset, &entries, has_more)))
}

/// One `/sessions` row: type/status marker, display title, recency
/// bucket (drives the divider labels), and the jump link (if any).
pub(crate) struct SessionEntry {
    pub(crate) marker: &'static str,
    pub(crate) title: String,
    pub(crate) bucket: usize,
    pub(crate) link: Option<String>,
}

/// Recency buckets for `/sessions` divider labels: 0 = 最近 6 小时,
/// 1 = 6 小时前, 2 = 一天前, 3 = 一周前.
pub(crate) fn session_time_bucket(
    updated: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> usize {
    let age = now.signed_duration_since(updated);
    if age < chrono::Duration::hours(6) {
        0
    } else if age < chrono::Duration::hours(24) {
        1
    } else if age < chrono::Duration::days(7) {
        2
    } else {
        3
    }
}

pub(crate) const SESSION_BUCKET_LABELS: [&str; 4] = ["", "6 小时前", "一天前", "一周前"];

/// The plain-text rendering of `/sessions` (fallback for platforms
/// without cards).
pub(crate) fn sessions_text(offset: usize, entries: &[SessionEntry], has_more: bool) -> String {
    let mut lines = vec![format!(
        "Recent sessions ({}–{}):",
        offset + 1,
        offset + entries.len()
    )];
    let mut bucket = None;
    for (i, e) in entries.iter().enumerate() {
        if bucket != Some(e.bucket) {
            bucket = Some(e.bucket);
            if e.bucket > 0 {
                lines.push(format!("── {} ──", SESSION_BUCKET_LABELS[e.bucket]));
            }
        }
        lines.push(match &e.link {
            // [title](url) is valid in both Telegram MarkdownV2 and
            // lark_md — `<a href>` would render literally on text paths.
            Some(link) => format!("{}. {} [{}]({link})", offset + i + 1, e.marker, e.title),
            None => format!("{}. {} {}", offset + i + 1, e.marker, e.title),
        });
    }
    if has_more {
        lines.push(format!(
            "Next page → `/sessions {}`",
            offset + SESSIONS_PAGE_SIZE
        ));
    }
    lines.join("\n")
}

/// The card rendering of `/sessions`: colored header, one row per
/// session (marker + bold linked title), recency dividers between
/// buckets, and a muted next-page hint when there is more.
pub(crate) fn sessions_card(offset: usize, entries: &[SessionEntry], has_more: bool) -> String {
    let mut elements: Vec<serde_json::Value> = Vec::new();
    let mut bucket = None;
    for e in entries {
        if bucket != Some(e.bucket) {
            bucket = Some(e.bucket);
            if e.bucket > 0 {
                if !elements.is_empty() {
                    elements.push(serde_json::json!({ "tag": "hr" }));
                }
                elements.push(serde_json::json!({
                    "tag": "markdown", "text_size": "notation",
                    "content": SESSION_BUCKET_LABELS[e.bucket]
                }));
            }
        }
        let title_md = match &e.link {
            Some(link) => format!("{} [**{}**]({link})", e.marker, e.title),
            None => format!("{} **{}**", e.marker, e.title),
        };
        elements.push(serde_json::json!({ "tag": "markdown", "content": title_md }));
    }
    if has_more {
        elements.push(serde_json::json!({ "tag": "hr" }));
        elements.push(serde_json::json!({
            "tag": "markdown", "text_size": "notation",
            "content": format!("下一页 → `/sessions {}`", offset + SESSIONS_PAGE_SIZE)
        }));
    }
    serde_json::json!({
        "schema": "2.0",
        "config": { "width_mode": "compact" },
        "header": {
            "template": "blue",
            "title": {"tag": "plain_text",
                "content": format!("📋 Recent sessions ({}–{})", offset + 1, offset + entries.len())},
        },
        "body": { "elements": elements },
    })
    .to_string()
}

/// The display title for a `/sessions` line: sanitized (see
/// [`sanitize_session_title`]) and capped.
pub(crate) fn session_link_title(info: &crate::storage::session::SessionInfo) -> String {
    sanitize_session_title(info.title.as_deref().unwrap_or(""))
}

/// Sanitize a session title for `/sessions` rendering: titles are
/// user-influenceable (first message, `/thread <topic>`, LLM titles,
/// rename API), and raw metacharacters would break the markup or inject
/// a foreign link. `<a href>` is the text-path markup and `[**..**](..)`
/// the card-path one, so both angle brackets and lark_md metacharacters
/// are full-width'd. Empty → `(untitled)`; capped at 30 chars.
pub(crate) fn sanitize_session_title(raw: &str) -> String {
    let title = raw.trim();
    if title.is_empty() {
        return "(untitled)".to_string();
    }
    let title: String = title
        .chars()
        .map(|c| match c {
            '<' => '＜',
            '>' => '＞',
            '[' => '［',
            ']' => '］',
            '(' => '（',
            ')' => '）',
            '*' => '＊',
            '`' => '｀',
            '~' => '～',
            _ => c,
        })
        .collect();
    let truncated: String = title.chars().take(30).collect();
    if title.chars().count() > 30 {
        format!("{truncated}…")
    } else {
        truncated
    }
}
