//! Slash-command handlers (`handle_incoming_message` and the
//! `/bind` `/mention` `/threads` `/sessions` blocks).

use crate::kernel::Kernel;

use crate::types::{ContentBlock, Result, SessionId};

use std::sync::Arc;
use tracing::warn;

use crate::channels::hub_command::{
    format_channel_line, format_current_model, format_model_list, format_rules,
    format_runtime_status, format_session_info, format_unknown_model, format_usage,
    format_watch_line, format_workflow_list, format_workflow_result, parse_channel_command,
    suggest_command, ChannelCommand, OverrideMode, HELP_TEXT, WORKFLOW_USAGE,
};
use crate::channels::hub_context::{append_message_images, prepare_trigger, TriggerKind};
use crate::channels::hub_deliver::send_info_reply;
use crate::channels::hub_routing::{
    command_reply_anchor, command_session_key, effective_mapping_key, get_or_create_session,
    history_container, is_chat_level_message, resolve_reply_in_thread, resolve_require_mention,
    session_jump_link, session_model_key, subscription_scope_key, thread_refusal, MentionSource,
};

use crate::channels::{
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
    let cmd = parse_channel_command(msg.raw_text.as_deref());
    let reply_msg_id = command_reply_anchor(&msg, rit, &cmd);
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

    match cmd {
        ChannelCommand::Help => {
            send_info_reply(adapter, &msg, reply_msg_id, "📖 Commands", HELP_TEXT.to_string())
                .await?;
            Ok(None)
        }
        ChannelCommand::Clear => {
            // Chat-level messages address the chat session
            // (command_session_key); with no session there, say so
            // instead of claiming success for a no-op.
            let key = command_session_key(&msg, rit, &chat_id, &mapping_key);
            let Some(sid) = store.find_mapping(channel_name, key).await? else {
                return Ok(Some("No session here yet — nothing to clear.".to_string()));
            };
            if let Err(e) = kernel.clear_session(&sid) {
                tracing::warn!("Failed to clear session {}: {}", sid.0, e);
            }
            Ok(Some("🧹 Context cleared.".to_string()))
        }
        ChannelCommand::Compact => {
            // Fire-and-forget: with observability the compact gets its own
            // status card (materialized on `Compacting`, settled by
            // `Compacted`) — that card is the feedback. Otherwise this
            // text ack is the only one (the outcome is only logged).
            let key = command_session_key(&msg, rit, &chat_id, &mapping_key);
            if let Some(sid) = store.find_mapping(channel_name, key).await? {
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
            let key = command_session_key(&msg, rit, &chat_id, &mapping_key);
            if let Some(sid) = store.find_mapping(channel_name, key).await? {
                kernel.cancel(&sid);
                return Ok(Some("⏹ Stopped.".to_string()));
            }
            Ok(Some("No active session to stop.".to_string()))
        }
        ChannelCommand::Restart => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
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
            // Deferred image download — as for a plain trigger, only
            // now, after the gate, does an attached image cost bandwidth.
            append_message_images(
                adapter,
                msg.external_message_id.as_deref().unwrap_or(""),
                &msg.image_keys,
                &mut blocks,
            )
            .await;
            kernel.send_message_inner(&sid, blocks, false).await?;
            Ok(None)
        }
        ChannelCommand::ListModels => {
            let models = kernel.list_models().await?;
            let current =
                session_model_key(channel_name, store, &kernel, &chat_id, &mapping_key).await?;
            send_info_reply(
                adapter,
                &msg,
                reply_msg_id,
                "📦 Available models",
                format_model_list(&models, &current),
            )
            .await?;
            Ok(None)
        }
        ChannelCommand::CurrentModel => {
            let models = kernel.list_models().await?;
            let current =
                session_model_key(channel_name, store, &kernel, &chat_id, &mapping_key).await?;
            send_info_reply(
                adapter,
                &msg,
                reply_msg_id,
                "🧠 Current model",
                format_current_model(&models, &current),
            )
            .await?;
            Ok(None)
        }
        ChannelCommand::SwitchModel(key) => {
            let models = kernel.list_models().await?;
            if !models.iter().any(|model| model.name == key) {
                return Ok(Some(format_unknown_model(&key, &models)));
            }
            // Anything outside a thread switches the whole chat (chat
            // session + thread fan-out) — a top-level quote-reply
            // addresses the chat, not a thread session (a fresh key of
            // its own reaches no live conversation). Inside a thread the
            // choice is always thread-scoped: a sessionless thread is
            // claimed as the override anchor (inheritance applies at
            // creation, the explicit key wins) — never fanned out.
            if msg.thread_id.is_none() {
                set_chat_model(channel_name, store, &kernel, &chat_id, Some(&key)).await?;
                // 行为一致（DM 里 fan-out 只有 chat session 一个目
                // 标），ack 按是否有 threads 可言区分措辞。
                return Ok(Some(if msg.is_group {
                    format!(
                        "Switched all threads in this chat to `{key}`. It takes effect on the next model invocation."
                    )
                } else {
                    format!(
                        "✅ Switched to `{key}`. It takes effect on the next model invocation."
                    )
                }));
            }
            let (sid, _) = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
                crate::channels::MappingKind::Normal,
            )
            .await?;
            kernel.set_session_model(&sid, &key).await?;
            Ok(Some(format!(
                "✅ Switched to `{key}`. It takes effect on the next model invocation."
            )))
        }
        ChannelCommand::InvalidModelCommand => Ok(Some(
            "Usage: `/model` or `/model <model_key>`. Use `/models` to list models.".to_string(),
        )),
        ChannelCommand::Mention(mode) => {
            handle_mention_command(config, store, &msg, adapter, reply_msg_id, mode).await
        }
        ChannelCommand::InvalidMentionCommand => Ok(Some(
            "Usage: `/mention` to show the current setting; `/mention on|off|reset` to change it (admin)."
                .to_string(),
        )),
        ChannelCommand::Threads(mode) => {
            handle_threads_command(config, store, &msg, adapter, reply_msg_id, mode).await
        }
        ChannelCommand::InvalidThreadsCommand => Ok(Some(
            "Usage: `/threads` to show the current setting; `/threads on|off|reset` to change it (admin)."
                .to_string(),
        )),
        ChannelCommand::Watch(on) => {
            handle_watch_command(config, store, &kernel, &msg, adapter, reply_msg_id, on).await
        }
        ChannelCommand::InvalidWatchCommand => Ok(Some(
            "Usage: `/watch` to show the current setting; `/watch on|off` to change it (admin)."
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
        ChannelCommand::Status => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            let cron_jobs = match kernel.cron_store() {
                Some(store) => match store.list_active().await {
                    Ok(jobs) => Some(jobs.len()),
                    Err(e) => {
                        warn!(error = %e, "cron job count failed");
                        None
                    }
                },
                None => None,
            };
            let channels = kernel
                .channel_manager
                .as_ref()
                .map(|hub| hub.list_channels())
                .unwrap_or_default();
            let body = format_runtime_status(
                kernel.started_at(),
                kernel.live_session_count(),
                kernel.list_all_background_shells().len(),
                kernel.list_all_running_subagents().await?.len(),
                cron_jobs,
                &channels,
            );
            send_info_reply(adapter, &msg, reply_msg_id, "🩺 Runtime", body).await?;
            Ok(None)
        }
        ChannelCommand::Usage(days) => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            let window = days as i64;
            let summary = kernel.get_usage_summary(window).await?;
            let daily = kernel.get_daily_usage(window).await?;
            let models = kernel.get_model_usage(window).await?;
            send_info_reply(
                adapter,
                &msg,
                reply_msg_id,
                &format!("📊 Usage · {days}d"),
                format_usage(days, &summary, &daily, &models),
            )
            .await?;
            Ok(None)
        }
        ChannelCommand::InvalidUsageCommand => Ok(Some(
            "Usage: `/usage [days]` — token usage for the last N days (default 7, max 90)."
                .to_string(),
        )),
        ChannelCommand::WorkflowList => {
            let data_dir = kernel.data_dir().await;
            let entries = crate::workflow::list(&data_dir).await?;
            let body = format_workflow_list(&crate::workflow::workflows_dir(&data_dir), &entries);
            send_info_reply(adapter, &msg, reply_msg_id, "🧩 Workflows", body).await?;
            Ok(None)
        }
        ChannelCommand::WorkflowRemove(name) => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            if crate::workflow::remove(&kernel.data_dir().await, &name).await? {
                Ok(Some(format!("🗑 Removed workflow `{name}`.")))
            } else {
                Ok(Some(format!(
                    "Workflow `{name}` not found — `/workflow ls` to list."
                )))
            }
        }
        ChannelCommand::WorkflowRun { name, args } => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            let data_dir = kernel.data_dir().await;
            let Some(path) = crate::workflow::resolve(&data_dir, &name).await? else {
                return Ok(Some(format!(
                    "Workflow `{name}` not found — `/workflow ls` to list."
                )));
            };
            if !crate::workflow::executable(&path).await {
                return Ok(Some(format!(
                    "`{name}` is not executable — fix with `chmod +x {}`.",
                    path.display()
                )));
            }
            // 脚本作用于当前会话的工作区（与 agent 所见一致）；没有会话
            // 时落默认 workspace。
            let key = command_session_key(&msg, rit, &chat_id, &mapping_key);
            let sid = store.find_mapping(channel_name, key).await?;
            let cwd = match &sid {
                Some(sid) => kernel.session_cwd(sid).await,
                None => crate::utils::path::session_workspace_dir(&data_dir, None),
            };
            // 异步执行 + 完成后回执：dispatch 循环串行，同步等待会阻塞
            // 本渠道后续消息（最长 RUN_TIMEOUT）。
            let adapter_bg = Arc::clone(adapter);
            let msg_bg = msg.clone();
            let reply_bg = reply_msg_id.clone();
            let name_bg = name.clone();
            let sid_str = sid.map(|s| s.0);
            tokio::spawn(async move {
                let outcome = crate::workflow::run(
                    &path,
                    &args,
                    &cwd,
                    &data_dir,
                    sid_str.as_deref(),
                    crate::workflow::RUN_TIMEOUT,
                )
                .await;
                let body = match outcome {
                    Ok(o) => format_workflow_result(&name_bg, &o),
                    Err(e) => format!("`{name_bg}` failed to start: {e}"),
                };
                if let Err(e) =
                    send_info_reply(&adapter_bg, &msg_bg, reply_bg, "🧩 Workflow", body).await
                {
                    warn!(error = %e, "workflow result reply failed");
                }
            });
            Ok(Some(format!(
                "▶️ Running `{name}`… result follows when done."
            )))
        }
        ChannelCommand::InvalidWorkflowCommand => Ok(Some(WORKFLOW_USAGE.to_string())),
        ChannelCommand::Mailbox(sub) => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            // Same scope resolution as `/info`: chat-level messages show
            // the chat session, in-thread ones the thread's.
            let chat_level = is_chat_level_message(&msg, rit);
            let key = command_session_key(&msg, rit, &chat_id, &mapping_key);
            let Some(sid) = store.find_mapping(channel_name, key).await? else {
                return Ok(Some(format!(
                    "No session yet in this {}.",
                    if chat_level { "chat" } else { "thread" },
                )));
            };
            crate::channels::mailbox::handle_mailbox_command(&kernel, adapter, &msg, reply_msg_id, &sid, sub)
                .await
        }
        ChannelCommand::InvalidMailboxCommand => Ok(Some(
            "Usage: `/mailbox` to show pending messages; `/mailbox retract <n>` · `/mailbox clear [steer|queue|all]` (admin)."
                .to_string(),
        )),
        ChannelCommand::Settings => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            crate::channels::settings::handle_settings_command(
                channel_name,
                config,
                &kernel,
                store,
                adapter,
                &msg,
                reply_msg_id,
            )
            .await
        }
        ChannelCommand::Cron => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            crate::channels::cron_card::handle_cron_command(&kernel, adapter, &msg, reply_msg_id).await
        }
        ChannelCommand::BackgroundTasks { all } => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            let chat_level = is_chat_level_message(&msg, rit);
            let key = command_session_key(&msg, rit, &chat_id, &mapping_key);
            let (sid, shells, subagents) = if all {
                let shells = kernel.list_all_background_shells();
                let subagents = kernel.list_all_running_subagents().await?;
                (None, shells, subagents)
            } else {
                let Some(sid) = store.find_mapping(channel_name, key).await? else {
                    return Ok(Some(format!(
                        "No session yet in this {}.",
                        if chat_level { "chat" } else { "thread" },
                    )));
                };
                let shells = kernel.list_background_shells(&sid);
                let subagents = running_subagents(&kernel, &sid).await;
                (Some(sid), shells, subagents)
            };
            if shells.is_empty() && subagents.is_empty() {
                return Ok(Some(if all {
                    "No background tasks.".to_string()
                } else {
                    "No background tasks in this session.".to_string()
                }));
            }
            if msg.doc_comment.is_none() && adapter.supports_status_card() {
                let card = background_tasks_card(sid.as_ref(), &shells, &subagents, all);
                adapter
                    .send_card(&msg.external_chat_id, &card, reply_msg_id.as_deref())
                    .await?;
                return Ok(None);
            }
            let mut lines = Vec::new();
            for (i, s) in shells.iter().enumerate() {
                lines.push(format!(
                    "- **#{}** ⚙️ `{}` · pid {} · {}",
                    i + 1,
                    s.command,
                    s.pid,
                    crate::storage::format_age(s.started_at)
                ));
            }
            for (i, s) in subagents.iter().enumerate() {
                lines.push(format!(
                    "- **#{}** 🤖 {} · {}",
                    shells.len() + i + 1,
                    s.alias.as_deref().unwrap_or("(untitled)"),
                    crate::storage::format_age(s.created_at)
                ));
            }
            let title = if all {
                "🖥 Background tasks (all sessions)"
            } else {
                "🖥 Background tasks"
            };
            send_info_reply(adapter, &msg, reply_msg_id, title, lines.join("\n")).await?;
            Ok(None)
        }
        ChannelCommand::Info => {
            // Chat-level messages show the chat session, in-thread ones
            // the thread's. Read-only: never creates a session or mapping.
            let chat_level = is_chat_level_message(&msg, rit);
            // Watch is chat-scoped: the line appears whenever /info runs
            // at the chat's top level (any reply_in_thread mode), never
            // inside a thread; a read failure degrades to no line, never
            // breaks /info.
            let top_level = msg.thread_id.is_none() && msg.root_id.is_none();
            let watch_line = if top_level {
                crate::channels::hub::watch::get_channel_watch_by_name(&store, channel_name, &chat_id)
                    .await
                    .ok()
                    .and_then(|st| format_watch_line(&st))
            } else {
                None
            };
            let key = command_session_key(&msg, rit, &chat_id, &mapping_key);
            // Channel line: the conversation's own ids (platform / chat /
            // thread) — the reference values allowlists, rules files and
            // subscriptions are written against. Shown in both branches.
            let channel_line = format_channel_line(config.platform.name(), channel_name, &msg);
            let Some(sid) = store.find_mapping(channel_name, key).await? else {
                let model_key =
                    session_model_key(channel_name, store, &kernel, &chat_id, key).await?;
                let mut text = format!(
                    "No session yet in this {}. First message will use `{model_key}`.\n\
                     - **Daemon**: yomi v{} · wire v{}\n{channel_line}",
                    if chat_level { "chat" } else { "thread" },
                    env!("CARGO_PKG_VERSION"),
                    crate::wire::WIRE_PROTOCOL_VERSION,
                );
                if let Some(line) = watch_line {
                    text = format!("{text}\n{line}");
                }
                return Ok(Some(text));
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
            let context_tokens = kernel.get_session_context_tokens(&sid).await;
            let mut body = format_session_info(
                &session,
                &model_key,
                &models,
                running_subagents,
                &shells,
                context_tokens,
            );
            // Meta block at the bottom: channel ids, then the (chat-scoped,
            // conditional) watch line adjacent to them.
            body = format!("{body}\n{channel_line}");
            if let Some(line) = watch_line {
                body = format!("{body}\n{line}");
            }
            send_info_reply(adapter, &msg, reply_msg_id, "ℹ️ Session info", body).await?;
            Ok(None)
        }
        ChannelCommand::Rules => {
            // Read-only: the same bounded reads the prompt assembly
            // uses, so the reply shows what a spawn of the addressed
            // session would inject. Never creates a session or mapping.
            //
            // Watch-on chats are the exception to scope keying: every
            // message (threaded or not) is mirrored to the chat-keyed
            // observer session, so that is the session whose rules are
            // in effect — a thread-scope lookup would falsely claim
            // "no session here yet".
            let key = command_session_key(&msg, rit, &chat_id, &mapping_key);
            let mut sid = store.find_mapping(channel_name, key).await?;
            if let Ok(status) = crate::channels::hub::watch::get_channel_watch_by_name(
                &store,
                channel_name,
                &chat_id,
            )
            .await
            {
                if status.on {
                    sid = status.session_id.map(crate::types::SessionId::from);
                }
            }
            let data_dir = kernel.data_dir().await;
            let channel_rules = crate::prompt::channel_rules_section(&data_dir, &chat_id).await;
            let session_rules = match &sid {
                Some(sid) => crate::prompt::session_rules_section(&data_dir, &sid.0).await,
                None => None,
            };
            let body = format_rules(
                channel_rules.as_deref(),
                session_rules.as_deref(),
                sid.as_deref(),
            );
            send_info_reply(adapter, &msg, reply_msg_id, "📜 Rules", body).await?;
            Ok(None)
        }
        ChannelCommand::Permits => {
            if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            let body = crate::channels::approval::pending_list_body(channel_name, store).await?;
            send_info_reply(adapter, &msg, reply_msg_id, "🔐 Pending approvals", body).await?;
            Ok(None)
        }
        ChannelCommand::Approve { id, perm } => {
            crate::channels::approval::approve(
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
            crate::channels::approval::deny(
                channel_name,
                config,
                store,
                adapter,
                &msg.external_user_id,
                id,
            )
            .await
        }
        ChannelCommand::InvalidApprovalCommand => Ok(Some(crate::channels::approval::usage())),
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
                "✅ Unsubscribed.".to_string()
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
                rit,
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
        ChannelCommand::Unknown(cmd) => Ok(Some(match suggest_command(&cmd) {
            Some(suggestion) => {
                format!("Unknown command `{cmd}`. Did you mean `{suggestion}`? See `/help`.")
            }
            None => format!("Unknown command `{cmd}`. See `/help` for the command list."),
        })),
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
            // adapter's metadata header ([ts][from: …]/[from_user_id: …]),
            // and context blocks merge ahead of it (see note_title_input).
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
/// Retargeting is admin-only. The scope follows `command_session_key`:
/// a chat-level command binds the chat itself (its message-id key
/// would bind a scope no follow-up ever reaches). A session already
/// routed elsewhere is refused: for chat scopes that means another
/// chat/channel (a reply could land in the wrong chat); for doc-comment
/// scopes, ANY other mapping (the delivery target comes from the
/// mapping row itself, so sharing across comment threads would post
/// answers to the wrong document). Unrouted sessions (GUI/CLI-created)
/// are free to adopt.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_bind(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    chat_id: &str,
    rit: bool,
    mapping_key: &str,
    reply_msg_id: Option<String>,
    target: Option<String>,
) -> Result<String> {
    let scope_key = command_session_key(msg, rit, chat_id, mapping_key);
    let current = store.find_mapping(channel_name, scope_key).await?;
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
    if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
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
        // A watch observer is bound to its chat's mirror stream by
        // construction; rebinding it would break the skill-only contract
        // (its delivery suppression follows the mapping's kind).
        if routing.is_watch() {
            return Ok(format!(
                "⚠️ `{target}` is a watch observer session; it cannot be rebound."
            ));
        }
        let compatible = if msg.doc_comment.is_some() {
            routing.mapping_key == scope_key
        } else {
            routing.channel_name == channel_name && routing.external_chat_id == chat_id
        };
        if !compatible {
            return Ok(format!(
                "⚠️ `{target}` is bound to another conversation; refusing to rebind."
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
            scope_key,
            &sid,
            chat_id,
            reply_msg_id.as_deref(),
            crate::channels::MappingKind::Normal,
        )
        .await?;
    // save_mapping writes `kind` only on row insert: rebinding a watched
    // chat preserves `watch` — the newly bound session becomes the
    // observer (watch follows the chat row, not a session).
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
    adapter: &Arc<dyn PlatformAdapter>,
    reply_msg_id: Option<String>,
    mode: Option<OverrideMode>,
) -> Result<Option<String>> {
    if !msg.is_group {
        return Ok(Some(
            "No need for this in DMs — every message is answered.".to_string(),
        ));
    }
    let on_off = |v: bool| if v { "on" } else { "off" };
    let container = history_container(msg);
    let scope = container.label();
    let Some(mode) = mode else {
        let (effective, source) = resolve_require_mention(store, config, msg).await;
        // The channel default is only a useful reference when an
        // override hides it; alone it just repeats itself.
        let suffix = if matches!(source, MentionSource::Default) {
            String::new()
        } else {
            format!(" · channel default: `{}`", on_off(config.require_mention))
        };
        let body = format!("This {scope}: `{}` ({source}){suffix}.", on_off(effective));
        send_info_reply(adapter, msg, reply_msg_id, "📣 Mention", body).await?;
        return Ok(None);
    };
    if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
        return Ok(Some(deny));
    }
    match mode {
        OverrideMode::On | OverrideMode::Off => {
            let value = matches!(mode, OverrideMode::On);
            store
                .set_mention_override(&config.name, container.id(), value)
                .await?;
            // The override governs conversation triggers only — commands
            // are control-plane and keep their own @ rule.
            let note = if value {
                ""
            } else {
                " Slash commands still need an @ in groups."
            };
            Ok(Some(format!(
                "✅ Mention requirement set to `{}` for this {scope} (channel default: `{}`).{note}",
                on_off(value),
                on_off(config.require_mention),
            )))
        }
        OverrideMode::Reset => {
            store
                .clear_mention_override(&config.name, container.id())
                .await?;
            let (effective, source) = resolve_require_mention(store, config, msg).await;
            Ok(Some(format!(
                "✅ Override cleared for this {scope}; now following {source}: `{}`.",
                on_off(effective),
            )))
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
    adapter: &Arc<dyn PlatformAdapter>,
    reply_msg_id: Option<String>,
    mode: Option<OverrideMode>,
) -> Result<Option<String>> {
    if !msg.is_group {
        return Ok(Some(
            "No need for this in DMs — replies are never threaded.".to_string(),
        ));
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
        // Same redundancy rule as /mention: the default is shown only
        // as an override's reference point.
        let suffix = if override_value.is_some() {
            format!(" · channel default: `{}`", on_off(config.reply_in_thread))
        } else {
            String::new()
        };
        let body = format!("This chat: `{}` ({source}){suffix}.", on_off(effective));
        send_info_reply(adapter, msg, reply_msg_id, "🧵 Reply-in-thread", body).await?;
        return Ok(None);
    };
    if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
        return Ok(Some(deny));
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
            Ok(Some(format!(
                "✅ Reply-in-thread set to `{}` for this chat (channel default: `{}`).{note}",
                on_off(value),
                on_off(config.reply_in_thread),
            )))
        }
        OverrideMode::Reset => {
            store.clear_rit_override(&config.name, chat_id).await?;
            Ok(Some(format!(
                "✅ Override cleared for this chat; now following the channel default: `{}`.",
                on_off(config.reply_in_thread),
            )))
        }
    }
}

/// `/watch`: query or switch this chat's watch mode (mutations are
/// admin-only). Chat-scoped. While on, every message of the chat is
/// mirrored to the chat's own session (kind `watch`, see `hub/watch.rs`)
/// — the group's only message consumer: conversation triggers are
/// suspended, and the agent itself decides when a reply is warranted,
/// speaking only via the platform skill from its own skill list (a pure
/// observer when none covers the platform).
///
/// The mapping's kind IS the switch: `/watch on` flips it to `watch`
/// (creating the session if absent); `/watch off` flips back to
/// `normal` — same session, same memory, only the mode changes. The
/// flip is a pure kind switch: nothing cancelled, nothing drained
/// (session continuity — see `watch::set_channel_watch_by_name`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_watch_command(
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    msg: &ChannelMessage,
    adapter: &Arc<dyn PlatformAdapter>,
    reply_msg_id: Option<String>,
    on: Option<bool>,
) -> Result<Option<String>> {
    if !msg.is_group {
        return Ok(Some(
            "No need for this in DMs — every message is answered.".to_string(),
        ));
    }
    if msg.thread_id.is_some() {
        return Ok(Some(
            "Watch applies to the whole chat — use `/watch` at top level.".to_string(),
        ));
    }
    let chat_id = &msg.external_chat_id;
    let Some(on) = on else {
        let watched = matches!(
            store.find_mapping_kind(&config.name, chat_id).await?,
            Some((_, crate::channels::MappingKind::Watch))
        );
        let body = if watched {
            "This chat: `on`.".to_string()
        } else {
            "This chat: `off`.".to_string()
        };
        send_info_reply(adapter, msg, reply_msg_id, "👁 Watch", body).await?;
        return Ok(None);
    };
    if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
        return Ok(Some(deny));
    }
    if on {
        // Eager create — or flip `normal` to `watch`: the chat's session
        // becomes its observer, memory intact (see set_channel_watch_by_name).
        crate::channels::hub::watch::set_channel_watch_by_name(
            store,
            kernel,
            &config.name,
            chat_id,
            true,
        )
        .await?;
        Ok(Some(crate::channels::hub::watch::flip_ack_text(true)))
    } else {
        // Flip back to `normal` — a pure kind switch: the same session
        // (with its in-flight run and queued work) answers mentions
        // again, watch-period memory intact.
        let status = crate::channels::hub::watch::set_channel_watch_by_name(
            store,
            kernel,
            &config.name,
            chat_id,
            false,
        )
        .await?;
        if status.session_id.is_none() {
            return Ok(Some("Watch is not on for this chat.".to_string()));
        }
        Ok(Some(crate::channels::hub::watch::flip_ack_text(false)))
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
    if let Some(deny) = crate::channels::approval::check_admin(config, &msg.external_user_id) {
        return Ok(Some(deny));
    }
    let (entries, has_more) =
        collect_session_entries(channel_name, store, kernel, adapter, offset).await?;
    if entries.is_empty() {
        return Ok(Some(if offset == 0 {
            "This channel has no sessions yet.".to_string()
        } else {
            format!("No more sessions beyond offset {offset}.")
        }));
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

pub(crate) const SESSION_BUCKET_LABELS: [&str; 4] = ["", "6h ago", "1d ago", "1w ago"];

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
    if has_more || offset > 0 {
        elements.push(serde_json::json!({ "tag": "hr" }));
        let mut cols = Vec::new();
        if offset > 0 {
            cols.push(serde_json::json!({
                "tag": "column", "width": "weighted", "weight": 1,
                "elements": [{
                    "tag": "button",
                    "text": { "tag": "plain_text", "content": "◀ Prev" },
                    "type": "default",
                    "size": "small",
                    "behaviors": [{ "type": "callback", "value": { "action": "pg_sessions", "offset": offset.saturating_sub(SESSIONS_PAGE_SIZE) } }],
                }],
            }));
        }
        if has_more {
            cols.push(serde_json::json!({
                "tag": "column", "width": "weighted", "weight": 1,
                "elements": [{
                    "tag": "button",
                    "text": { "tag": "plain_text", "content": "Next ▶" },
                    "type": "default",
                    "size": "small",
                    "behaviors": [{ "type": "callback", "value": { "action": "pg_sessions", "offset": offset + SESSIONS_PAGE_SIZE } }],
                }],
            }));
        }
        elements.push(serde_json::json!({ "tag": "column_set", "columns": cols }));
    }
    crate::channels::hub_deliver::info_card_envelope(
        &format!(
            "📋 Recent sessions ({}–{})",
            offset + 1,
            offset + entries.len()
        ),
        elements,
    )
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

/// Chat-scope model switch: update the chat-level session and every
/// existing thread session routed to this chat (future threads inherit
/// via the chat session's model key). `None` clears the override so the
/// chat follows the configured default again. Shared by `/model <key>`
/// (chat level) and the settings card.
pub(crate) async fn set_chat_model(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    chat_id: &str,
    key: Option<&str>,
) -> Result<()> {
    let (chat_sid, _) = get_or_create_session(
        channel_name,
        store,
        kernel,
        chat_id,
        chat_id,
        None,
        crate::channels::MappingKind::Normal,
    )
    .await?;
    match key {
        Some(k) => kernel.set_session_model(&chat_sid, k).await?,
        None => kernel.clear_session_model(&chat_sid).await?,
    }
    for (mk, sid) in store.list_mappings(channel_name).await? {
        if mk == chat_id {
            continue;
        }
        if let Ok(Some(routing)) = store.find_routing_by_session(&sid).await {
            if routing.external_chat_id == chat_id {
                // 个别 session 失败（并发删除/陈旧 mapping）不中断扇出——
                // 写得进去的写，失败仅告警。
                let r = match key {
                    Some(k) => kernel.set_session_model(&sid, k).await,
                    None => kernel.clear_session_model(&sid).await,
                };
                if let Err(e) = r {
                    warn!(channel = %channel_name, chat_id, session_id = %sid.0, error = %e, "fan-out model switch skipped a session");
                }
            }
        }
    }
    Ok(())
}

/// Chat-scope context-window override：与 [`set_chat_model`] 同扇出——写
/// chat session 与该 chat 现存的全部 thread session（未来 thread 建行
/// 时经 `overrides_for_new_channel_session` 继承）。`None` 清除覆盖，
/// 回落跟随模型配置。Shared by the settings card（channel 侧唯一入口；
/// 精确值走 GUI/TUI/CLI）。
pub(crate) async fn set_chat_context_window(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    chat_id: &str,
    tokens: Option<u32>,
) -> Result<()> {
    let (chat_sid, _) = get_or_create_session(
        channel_name,
        store,
        kernel,
        chat_id,
        chat_id,
        None,
        crate::channels::MappingKind::Normal,
    )
    .await?;
    kernel.set_session_context_window(&chat_sid, tokens).await?;
    for (mk, sid) in store.list_mappings(channel_name).await? {
        if mk == chat_id {
            continue;
        }
        if let Ok(Some(routing)) = store.find_routing_by_session(&sid).await {
            if routing.external_chat_id == chat_id {
                // 与 model 扇出同规：个别 session 失败不中断。
                if let Err(e) = kernel.set_session_context_window(&sid, tokens).await {
                    warn!(channel = %channel_name, chat_id, session_id = %sid.0, error = %e, "fan-out ctx switch skipped a session");
                }
            }
        }
    }
    Ok(())
}

/// `/sessions` 一页的条目收集（命令与翻页回调共用）：channel-routed
/// 过滤、游标扫描、并发取跳转链接、⚡/🧵/💬 标记与分桶。
async fn collect_session_entries(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    offset: usize,
) -> Result<(Vec<SessionEntry>, bool)> {
    // Channel-routed sessions only — a jump needs a delivery target.
    // Actively-watched chats' sessions (kind `watch`) get the 👁 marker.
    let routed: std::collections::HashSet<String> = store
        .list_mappings(channel_name)
        .await?
        .into_iter()
        .map(|(_, sid)| sid.0.to_string())
        .collect();
    let watchers: std::collections::HashSet<String> = store
        .list_watch_sessions(channel_name)
        .await?
        .into_iter()
        .map(|sid| sid.0.to_string())
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
        } else if watchers.contains(info.id.0.as_str()) {
            "👁"
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
    Ok((entries, has_more))
}

/// `pg_sessions` 翻页回调（/sessions 卡底部 ◀ ▶）：重取目标页并原地
/// 刷新。admin 门槛（与 /sessions 命令同档）。
pub(crate) async fn handle_sessions_action(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: &crate::channels::CardAction,
) {
    if let Some(deny) = crate::channels::approval::check_admin(config, &action.operator_open_id) {
        crate::channels::approval::send_action_denial(adapter, action, deny).await;
        return;
    }
    let Some(message_id) = &action.message_id else {
        return;
    };
    let offset = action.value["offset"].as_u64().unwrap_or(0) as usize;
    let page = collect_session_entries(channel_name, store, kernel, adapter, offset).await;
    let card = match page {
        Ok((entries, has_more)) if !entries.is_empty() => sessions_card(offset, &entries, has_more),
        Ok(_) => crate::channels::hub_deliver::info_card_envelope(
            "📋 Recent sessions",
            vec![serde_json::json!({
                "tag": "markdown", "text_size": "notation",
                "content": format!("No more sessions beyond offset {offset}."),
            })],
        ),
        Err(e) => {
            warn!(channel = %channel_name, error = %e, "sessions page fetch failed");
            return;
        }
    };
    if let Err(e) = adapter.update_card(message_id, &card).await {
        warn!(channel = %channel_name, error = %e, "sessions page card refresh failed");
    }
}

/// 当前 session 运行中的 subagent（`is_running` 过滤——列表管理面只
/// 关心活着的）。
async fn running_subagents(
    kernel: &Kernel,
    sid: &SessionId,
) -> Vec<crate::types::SubagentResponse> {
    match kernel.list_subagents(sid).await {
        Ok(subs) => subs.into_iter().filter(|s| s.is_running).collect(),
        Err(e) => {
            warn!(error = %e, "list subagents failed");
            Vec::new()
        }
    }
}

/// `/bg` 后台任务卡：shell 行（⚙️ command · pid · age）+ subagent 行
/// （🤖 任务名 · age），行尾 ⏹（shell=SIGTERM 进程组，sub=cancel），
/// 底部 🔄 Refresh；`all` 时跨 session 行尾灰字标注归属短 sid，所有
/// 回调 value 都带 `all`（刷新保持原视图）。卡片与 `bg_*` 回调共用
/// 同一渲染——操作后原地刷新就是重跑它。
pub(crate) fn background_tasks_card(
    sid: Option<&SessionId>,
    shells: &[crate::agent::BackgroundShellTask],
    subagents: &[crate::types::SubagentResponse],
    all: bool,
) -> String {
    let mut elements = Vec::new();
    let mut rows: Vec<(String, serde_json::Value)> = Vec::new();
    let owner = |owner_sid: &SessionId| {
        if all {
            format!(
                " <font color='grey'>[{}]</font>",
                &owner_sid.0[..12.min(owner_sid.0.len())]
            )
        } else {
            String::new()
        }
    };
    for s in shells.iter().take(20) {
        rows.push((
            format!(
                "⚙️ `{}` · pid {} · {}{}",
                s.command,
                s.pid,
                crate::storage::format_age(s.started_at),
                owner(&s.session_id),
            ),
            serde_json::json!({ "action": "bg_kill_shell", "sid": s.session_id.0, "task": s.task_id, "all": all }),
        ));
    }
    for s in subagents
        .iter()
        .take(20usize.saturating_sub(rows.len().min(20)))
    {
        rows.push((
            format!(
                "🤖 {} · {}{}",
                s.alias.as_deref().unwrap_or("(untitled)"),
                crate::storage::format_age(s.created_at),
                owner(&s.parent_session_id),
            ),
            serde_json::json!({ "action": "bg_stop_sub", "sid": s.id.0, "parent": s.parent_session_id.0, "all": all }),
        ));
    }
    let capped = rows.len() == 20 && shells.len() + subagents.len() > 20;
    for (i, (content, value)) in rows.into_iter().enumerate() {
        elements.push(serde_json::json!({
            "tag": "column_set",
            "columns": [
                {
                    "tag": "column", "width": "weighted", "weight": 1, "vertical_align": "center",
                    "elements": [{
                        "tag": "markdown", "text_size": "notation",
                        "content": format!("**#{}** {content}", i + 1),
                    }],
                },
                {
                    "tag": "column", "width": "auto", "vertical_align": "center",
                    "elements": [{
                        "tag": "button",
                        "text": { "tag": "plain_text", "content": "⏹" },
                        "type": "text",
                        "size": "small",
                        "behaviors": [{ "type": "callback", "value": value }],
                    }],
                },
            ],
        }));
    }
    if capped {
        elements.push(serde_json::json!({
            "tag": "markdown", "text_size": "notation",
            "content": "<font color='grey'>… capped at 20 rows</font>",
        }));
    }
    let refresh_value = match sid {
        Some(sid) => serde_json::json!({ "action": "bg_refresh", "sid": sid.0, "all": all }),
        None => serde_json::json!({ "action": "bg_refresh", "all": true }),
    };
    elements.push(serde_json::json!({
        "tag": "column_set",
        "columns": [{
            "tag": "column", "width": "weighted", "weight": 1,
            "elements": [{
                "tag": "button",
                "text": { "tag": "plain_text", "content": "🔄 Refresh" },
                "type": "default",
                "size": "small",
                "behaviors": [{ "type": "callback", "value": refresh_value }],
            }],
        }],
    }));
    let title = if all {
        "🖥 Background tasks (all sessions)"
    } else {
        "🖥 Background tasks"
    };
    crate::channels::hub_deliver::info_card_envelope(title, elements)
}

/// `bg_*` 按钮回调（/bg 卡行尾 ⏹ 与底部 🔄）：shell=SIGTERM 进程组，
/// sub=cancel 该 subagent session，然后原地刷新列表。停止动作只需
/// 路由层的 user 门限（与 /stop 同档），不叠加 admin。
pub(crate) async fn handle_bg_action(
    channel_name: &str,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    action: &crate::channels::CardAction,
) {
    let value = &action.value;
    match value["action"].as_str() {
        Some("bg_kill_shell") => {
            let sid = SessionId::from(value["sid"].as_str().unwrap_or_default().to_string());
            let task = value["task"].as_str().unwrap_or_default();
            if sid.0.is_empty() || task.is_empty() {
                warn!(value = %value, "bg kill action missing sid/task");
                return;
            }
            if !kernel.kill_background_shell(&sid, task).await {
                warn!(channel = %channel_name, task, "bg kill: task unknown or SIGTERM failed");
            }
            // Give the process group a moment to actually die before
            // re-listing (the tracker's cleanup rides the guard's Drop).
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        Some("bg_stop_sub") => {
            let sub_sid = SessionId::from(value["sid"].as_str().unwrap_or_default().to_string());
            if sub_sid.0.is_empty() {
                warn!(value = %value, "bg stop action missing sid");
                return;
            }
            kernel.cancel(&sub_sid);
        }
        Some("bg_refresh") => {}
        other => {
            warn!(value = %value, "unrecognized bg card action {other:?}");
            return;
        }
    }
    let Some(message_id) = &action.message_id else {
        return;
    };
    let all = value["all"].as_bool().unwrap_or(false);
    // The session the refreshed list belongs to (drives both the listing
    // and the next Refresh button's scope): `parent` for bg_stop_sub
    // (whose `sid` is the subagent itself), otherwise `sid`.
    let owner_sid = if value["action"].as_str() == Some("bg_stop_sub") {
        SessionId::from(value["parent"].as_str().unwrap_or_default().to_string())
    } else {
        SessionId::from(value["sid"].as_str().unwrap_or_default().to_string())
    };
    let (shells, subagents) = if all {
        let shells = kernel.list_all_background_shells();
        let subs = match kernel.list_all_running_subagents().await {
            Ok(s) => s,
            Err(e) => {
                // Keep the old card rather than painting a misleading
                // empty state over a transient store error.
                warn!(channel = %channel_name, error = %e, "bg --all subagent listing failed");
                return;
            }
        };
        (shells, subs)
    } else {
        if owner_sid.0.is_empty() {
            return;
        }
        let shells = kernel.list_background_shells(&owner_sid);
        let subs = running_subagents(kernel, &owner_sid).await;
        (shells, subs)
    };
    let card = if shells.is_empty() && subagents.is_empty() {
        crate::channels::hub_deliver::info_card_envelope(
            if all {
                "🖥 Background tasks (all sessions)"
            } else {
                "🖥 Background tasks"
            },
            vec![serde_json::json!({
                "tag": "markdown", "text_size": "notation",
                "content": "No background tasks.",
            })],
        )
    } else {
        let sid_ref = (!all && !owner_sid.0.is_empty()).then_some(&owner_sid);
        background_tasks_card(sid_ref, &shells, &subagents, all)
    };
    if let Err(e) = adapter.update_card(message_id, &card).await {
        warn!(channel = %channel_name, error = %e, "bg card refresh failed");
    }
}
