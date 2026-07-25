//! Update method for handling messages

use tuirealm::{
    props::{AttrValue, Attribute},
    state::{State, StateValue},
    terminal::TerminalAdapter,
};

use std::sync::Arc;

use crate::{
    attr,
    components::{default_help_sections, info_bar::Notification, PickerItem},
    id::Id,
    msg::Msg,
};
use kernel::event::Command;
use kernel::permission::Level;

use super::types::{AppMode, Model};

impl Model {
    pub async fn update(&mut self, msg: Option<Msg>) -> Option<Msg> {
        if let Some(msg) = msg {
            self.state.should_redraw = true;

            match msg {
                // Every user submission wraps its raw text here for history.
                // We save the text and then forward to the actual message.
                Msg::InputEntry(raw, inner) => {
                    if !raw.trim().is_empty() {
                        self.input_history.retain(|h| h != &raw);
                        self.input_history.push(raw.clone());
                        self.new_history_entries.retain(|h| h != &raw);
                        self.new_history_entries.push(raw);

                        let limit = crate::INPUT_HISTORY_LIMIT;
                        if self.input_history.len() > limit {
                            self.input_history = self
                                .input_history
                                .split_off(self.input_history.len() - limit / 2);
                        }
                        if self.new_history_entries.len() > limit {
                            self.new_history_entries = self
                                .new_history_entries
                                .split_off(self.new_history_entries.len() - limit / 2);
                        }

                        let _ = self.init_input_history();
                    }
                    Box::pin(self.update(Some(*inner))).await
                }
                Msg::Quit => {
                    self.state.quit = true;
                    None
                }
                // Ignore input-related messages in Browse mode
                Msg::InputSubmit(blocks) => {
                    if self.mode == AppMode::Browse {
                        return None;
                    }

                    // Check if we're currently streaming
                    if self.state.is_streaming {
                        // Queue the message to be sent when streaming ends (only one allowed)
                        self.set_queued_message(blocks);
                    } else if let Err(e) = self.input_tx.try_send(blocks) {
                        tracing::warn!("Failed to send input: {}", e);
                        self.show_notification(&Notification::error(
                            "Failed to send message. Session may be disconnected.",
                            5000,
                        ));
                    }
                    None
                }
                Msg::InputEnterEmpty => {
                    // "Enter again" gesture: empty input steers the queued message
                    if self.queued_message.is_some() {
                        self.steer_queued_message();
                    }
                    None
                }
                // Scrolling - works in both modes
                Msg::ScrollUp => {
                    let amount = if self.mode == AppMode::Browse { 1 } else { 3 };
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_UP),
                        AttrValue::Number(amount as isize),
                    );
                    None
                }
                Msg::ScrollDown => {
                    let amount = if self.mode == AppMode::Browse { 1 } else { 3 };
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_DOWN),
                        AttrValue::Number(amount as isize),
                    );
                    None
                }
                Msg::InputChanged(_) => {
                    // Ignore input changes in Browse mode
                    if self.mode == AppMode::Browse {
                        return None;
                    }
                    // Note: InputChanged is sent by InputComponent but doesn't need special handling here
                    // It's mainly used for tracking input state if needed
                    None
                }
                Msg::CancelRequest => {
                    let _ = self.ctrl_tx.try_send(Command::Cancel);
                    None
                }
                Msg::RecallQueuedMessage => {
                    self.recall_queued_message();
                    None
                }
                Msg::SteerQueuedMessage => {
                    self.steer_queued_message();
                    None
                }
                Msg::Redraw => {
                    self.state.should_redraw = true;
                    None
                }
                Msg::Notification(msg) => {
                    self.show_notification(&msg);
                    None
                }
                // Mode switching
                Msg::ToggleBrowseMode => {
                    match self.mode {
                        AppMode::Normal => {
                            // Enter browse mode
                            self.mode = AppMode::Browse;
                            // Update status bar to show BROWSE mode
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::SET_MODE),
                                AttrValue::Number(1),
                            );
                            // Update input box mode so it knows to use browse shortcuts
                            let _ = self.app.attr(
                                &Id::InputBox,
                                Attribute::Custom(attr::MODE),
                                AttrValue::Number(1),
                            );
                            // Show browse mode shortcuts in info bar
                            self.show_notification(&Notification::info(
                                "Browse: j/k scroll · u/d page · g/G top/bottom · C-e expand · q/C-o exit",
                                0,
                            ));
                            // Scroll progress will be updated in view() on next redraw
                        }
                        AppMode::Browse => {
                            // Exit browse mode
                            self.mode = AppMode::Normal;
                            // Collapse all blocks
                            let _ = self.app.attr(
                                &Id::ChatView,
                                Attribute::Custom(attr::COLLAPSE_ALL),
                                AttrValue::Flag(true),
                            );
                            // Update status bar to show NORMAL mode
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::SET_MODE),
                                AttrValue::Number(0),
                            );
                            // Update input box mode so it uses normal text input
                            let _ = self.app.attr(
                                &Id::InputBox,
                                Attribute::Custom(attr::MODE),
                                AttrValue::Number(0),
                            );
                            // Clear scroll progress (restore context usage display)
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::CLEAR_SCROLL_PROGRESS),
                                AttrValue::Flag(true),
                            );
                            // Clear browse mode notification
                            let _ = self.app.attr(
                                &Id::InfoBar,
                                Attribute::Custom(attr::CLEAR_NOTIFICATION),
                                AttrValue::Flag(true),
                            );
                        }
                    }
                    None
                }
                Msg::PageHalfUp => {
                    let height = self
                        .terminal
                        .raw()
                        .size()
                        .map_or(20, |s| (s.height / 2) as usize);
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::PAGE_UP),
                        AttrValue::Number(height as isize),
                    );
                    None
                }
                Msg::PageHalfDown => {
                    let height = self
                        .terminal
                        .raw()
                        .size()
                        .map_or(20, |s| (s.height / 2) as usize);
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::PAGE_DOWN),
                        AttrValue::Number(height as isize),
                    );
                    None
                }
                Msg::GoToTop => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_TO_TOP),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::GoToBottom => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_TO_BOTTOM),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::ToggleExpandAll => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::TOGGLE_EXPAND_ALL),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::DialogSelected(idx) => {
                    // Priority: ask_user > permission request
                    if self.pending_ask_user.is_some() {
                        let answer =
                            self.pending_ask_user
                                .as_ref()
                                .and_then(|(_, _, questions, _)| {
                                    questions.front().and_then(|q| {
                                        q.options.get(idx).map(|opt| opt.label.clone())
                                    })
                                });
                        self.advance_ask_user(answer);
                    } else if let Some((req_id, session_id)) = self.pending_permission.take() {
                        let (approved, remember) = match idx {
                            0 => (true, false), // Approve once
                            1 => (true, true),  // Always approve this tool
                            3 => {
                                // YOLO mode - enable Dangerous level
                                self.permission_level = Level::Dangerous;
                                // Update status bar to show YOLO
                                let _ = self.app.attr(
                                    &Id::StatusBar,
                                    Attribute::Custom(attr::SET_PERMISSION_LEVEL),
                                    AttrValue::Number(2),
                                );
                                // Show notification
                                self.show_notification(&Notification::info(
                                    "YOLO mode enabled - all tools will be auto-approved",
                                    5000,
                                ));
                                // Send command to kernel to update permission level
                                let _ = self.ctrl_tx.try_send(Command::SetLevel(Level::Dangerous));
                                (true, false)
                            }
                            _ => (false, false), // Deny
                        };
                        let coord = Arc::clone(&self.kernel);
                        let sid = kernel::types::SessionId::from(session_id);
                        tokio::spawn(async move {
                            if let Err(e) = coord
                                .send_permission_response(&sid, &req_id, approved, remember)
                                .await
                            {
                                tracing::error!("Failed to send permission response: {}", e);
                            }
                        });
                        // Return focus to input box
                        self.set_focus(&Id::InputBox);
                    } else {
                        // Dialog was closed but no pending request exists (e.g. ack arrived
                        // before the user responded). Restore focus so input doesn't get stuck.
                        self.set_focus(&Id::InputBox);
                    }
                    None
                }
                Msg::DialogCustomInput(text) => {
                    if self.pending_ask_user.is_some() {
                        self.advance_ask_user(Some(text));
                    } else {
                        // Same as DialogSelected: if the request was already acked, we still
                        // need to restore focus so the input box doesn't get stuck.
                        self.set_focus(&Id::InputBox);
                    }
                    None
                }
                Msg::DialogCancelled => {
                    // Priority: ask_user > permission request
                    if self.pending_ask_user.is_some() {
                        self.cancel_ask_user();
                    } else if let Some((req_id, session_id)) = self.pending_permission.take() {
                        let coord = Arc::clone(&self.kernel);
                        let sid = kernel::types::SessionId::from(session_id);
                        tokio::spawn(async move {
                            if let Err(e) = coord
                                .send_permission_response(&sid, &req_id, false, false)
                                .await
                            {
                                tracing::error!("Failed to send permission deny response: {}", e);
                            }
                        });
                        // Return focus to input box
                        self.set_focus(&Id::InputBox);
                    }
                    None
                }
                // Slash commands
                Msg::CommandNew => {
                    // Signal that a new session should be created
                    self.state.should_create_new_session = true;
                    self.state.quit = true;
                    None
                }
                Msg::CommandFork => {
                    // Fork current session and switch to the new one
                    let coord = Arc::clone(&self.kernel);
                    let tx = self.cmd_tx.clone();
                    let sid = kernel::types::SessionId::from(self.session_id.clone());
                    let level = self.permission_level;
                    tokio::spawn(async move {
                        let msg = match coord.fork_session(&sid, level).await {
                            Ok(new_id) => Msg::SessionSelected(new_id.0.to_string()),
                            Err(e) => Msg::Notification(Notification::error(
                                format!("Fork failed: {e}"),
                                5000,
                            )),
                        };
                        if let Err(e) = tx.send(msg) {
                            tracing::debug!("cmd channel closed, dropping async result: {e}");
                        }
                    });
                    None
                }
                Msg::CommandContinue => {
                    let _ = self.ctrl_tx.try_send(Command::Continue);
                    self.show_notification(&Notification::info("Agent continuing...", 3000));
                    None
                }
                Msg::CommandGoal(description) => {
                    let state = kernel::goal::GoalState::new(description);
                    let _ = self.ctrl_tx.try_send(Command::StartGoal(state));
                    self.show_notification(&Notification::info(
                        "Goal mode activated. Agent will work autonomously. Use /goal:stop to interrupt.",
                        5000,
                    ));
                    None
                }
                Msg::CommandGoalStop => {
                    let _ = self.ctrl_tx.try_send(Command::StopGoal);
                    self.show_notification(&Notification::info(
                        "Goal mode stopped. Agent will wait for your input.",
                        3000,
                    ));
                    None
                }
                Msg::CommandTodos => {
                    // Toggle todo list visibility
                    let _ = self.app.attr(
                        &Id::TodoList,
                        Attribute::Custom(attr::TOGGLE_TODOS),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::CommandYolo => {
                    // Toggle YOLO mode via command
                    Box::pin(self.update(Some(Msg::ToggleYoloMode))).await
                }
                Msg::ToggleYoloMode => {
                    // Toggle between Safe and Dangerous permission levels
                    let new_level = if self.permission_level == Level::Dangerous {
                        Level::Safe
                    } else {
                        Level::Dangerous
                    };
                    self.permission_level = new_level;

                    // Update status bar
                    let level_num = match new_level {
                        Level::Safe => 0,
                        Level::Caution => 1,
                        Level::Dangerous => 2,
                    };
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom(attr::SET_PERMISSION_LEVEL),
                        AttrValue::Number(level_num),
                    );

                    // Show status message
                    let msg = if new_level == Level::Dangerous {
                        "YOLO mode enabled - all tools will be auto-approved"
                    } else {
                        "YOLO mode disabled"
                    };
                    self.show_notification(&Notification::info(msg, 5000));

                    // Send command to kernel
                    let _ = self.ctrl_tx.try_send(Command::SetLevel(new_level));

                    None
                }
                Msg::CommandBrowse => {
                    // Toggle browse mode
                    Box::pin(self.update(Some(Msg::ToggleBrowseMode))).await
                }
                Msg::CommandCompact => {
                    // Send compact request
                    let _ = self.ctrl_tx.try_send(Command::Compact);
                    self.show_notification(&Notification::info("Compacting messages...", 3000));
                    None
                }
                Msg::CommandSteer(blocks) => {
                    let _ = self.ctrl_tx.try_send(Command::Steer { content: blocks });
                    self.show_notification(&Notification::info(
                        "Steer message queued for next step",
                        3000,
                    ));
                    None
                }
                Msg::Suspend => {
                    // Suspend process to background (Ctrl-Z)
                    self.suspend_process();
                    None
                }
                Msg::ReadClipboard => {
                    #[cfg(not(target_os = "macos"))]
                    {
                        let handle = tokio::task::spawn_blocking(|| {
                            use arboard::Clipboard;
                            Clipboard::new().ok().and_then(|mut c| c.get_text().ok())
                        });
                        self.clipboard_handle = Some(handle);
                    }
                    None
                }
                Msg::ClipboardText(text) => {
                    let _ = self.app.attr(
                        &Id::InputBox,
                        Attribute::Custom(attr::CLIPBOARD_PASTE),
                        AttrValue::String(text),
                    );
                    // Trigger InputChanged so completion updates
                    let content = match self.app.state(&Id::InputBox) {
                        Ok(State::Single(StateValue::String(c))) => c,
                        _ => String::new(),
                    };
                    Some(Msg::InputChanged(content))
                }
                // History picker messages
                Msg::ShowHistoryPicker => {
                    // Convert history to picker items (most recent first)
                    let items = self.history_items();
                    // Show the picker with history items
                    let _ = self.app.attr(
                        &Id::HistoryPicker,
                        Attribute::Custom(attr::PICKER_ITEMS),
                        AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(items))),
                    );
                    let _ = self.app.attr(
                        &Id::HistoryPicker,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::Flag(true),
                    );
                    // Give focus to history picker
                    self.set_focus(&Id::HistoryPicker);
                    None
                }
                Msg::HistorySelected(idx_str) => {
                    // Extract the actual index from "history_{idx}"
                    if let Some(idx_part) = idx_str.strip_prefix("history_") {
                        if let Ok(idx) = idx_part.parse::<usize>() {
                            if idx < self.input_history.len() {
                                let selected_text = self.input_history[idx].clone();
                                // Set the input box content using custom attribute
                                let _ = self.app.attr(
                                    &Id::InputBox,
                                    Attribute::Custom(attr::INPUT_CONTENT),
                                    AttrValue::String(selected_text),
                                );
                            }
                        }
                    }
                    // Return focus to input box and trigger redraw
                    self.set_focus(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CloseHistoryPicker => {
                    // Return focus to input box and trigger redraw
                    self.set_focus(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                // Help dialog messages
                Msg::CommandHelp => {
                    // Show help dialog with default help sections
                    let sections = default_help_sections();
                    if let Err(e) = self.app.attr(
                        &Id::HelpDialog,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(sections))),
                    ) {
                        tracing::warn!("Failed to show help dialog: {}", e);
                    }
                    // Give focus to help dialog so it receives keyboard events
                    self.set_focus(&Id::HelpDialog);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CommandSessions => {
                    let coord = Arc::clone(&self.kernel);
                    let tx = self.cmd_tx.clone();
                    let working_dir = self.working_dir.to_string_lossy().to_string();
                    tokio::spawn(async move {
                        let result = coord
                            .list_sessions(
                                None,
                                kernel::storage::session::SessionListScope::All,
                                None,
                                200,
                            )
                            .await;
                        let sessions: Vec<_> = match result {
                            Ok(paginated) => paginated
                                .sessions
                                .into_iter()
                                .filter(|s| {
                                    s.working_dir.as_ref().is_some_and(|wd| wd == &working_dir)
                                })
                                .take(50)
                                .collect(),
                            Err(e) => {
                                tracing::warn!("Failed to list sessions: {e}");
                                Vec::new()
                            }
                        };

                        let items: Vec<PickerItem> = sessions
                            .into_iter()
                            .map(|s| {
                                let age_str = s.format_age();
                                let preview = s
                                    .title
                                    .unwrap_or_else(|| "(no user message)".to_string())
                                    .replace('\n', " ");
                                let id_str = s.id.0;
                                let short_id = super::types::format_short_id(&id_str);
                                let label = format!("{short_id} - {age_str}");
                                PickerItem::new(id_str, label).with_meta(preview)
                            })
                            .collect();
                        if let Err(e) = tx.send(Msg::SessionList(items)) {
                            tracing::debug!("cmd channel closed, dropping session list: {e}");
                        }
                    });
                    None
                }
                Msg::SessionList(items) => {
                    if let Err(e) = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::PICKER_ITEMS),
                        AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(items))),
                    ) {
                        tracing::warn!("Failed to set session picker items: {}", e);
                    }
                    if let Err(e) = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::Flag(true),
                    ) {
                        tracing::warn!("Failed to show session picker: {}", e);
                    }
                    self.set_focus(&Id::SessionPicker);
                    self.state.should_redraw = true;
                    None
                }
                Msg::SessionSelected(session_id) => {
                    // Hide picker and set switch target
                    let _ = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    self.state.switch_to_session = Some(session_id);
                    self.state.quit = true;
                    None
                }
                Msg::CloseSessionPicker => {
                    // Hide session picker and return focus to input box
                    let _ = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    self.set_focus(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CommandModels(key) => {
                    if let Some(key) = key {
                        // Direct switch: /models <key>
                        return Some(Msg::ModelSelected(key));
                    }
                    let coord = Arc::clone(&self.kernel);
                    let tx = self.cmd_tx.clone();
                    let session_id = self.session_id.clone();
                    tokio::spawn(async move {
                        let sid = kernel::types::SessionId::from(session_id);
                        let models = match coord.list_models().await {
                            Ok(models) => models,
                            Err(e) => {
                                tracing::warn!("Failed to list models: {e}");
                                Vec::new()
                            }
                        };
                        let current = coord.get_session_model(&sid).await.unwrap_or_default();
                        let items = super::types::model_picker_items(&models, &current);
                        if let Err(e) = tx.send(Msg::ModelList(items)) {
                            tracing::debug!("cmd channel closed, dropping model list: {e}");
                        }
                    });
                    None
                }
                Msg::ModelList(items) => {
                    if items.is_empty() {
                        self.show_notification(&Notification::warn(
                            "No models configured (check [[models]] in config.toml)",
                            3000,
                        ));
                        return None;
                    }
                    if let Err(e) = self.app.attr(
                        &Id::ModelPicker,
                        Attribute::Custom(attr::PICKER_ITEMS),
                        AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(items))),
                    ) {
                        tracing::warn!("Failed to set model picker items: {}", e);
                    }
                    if let Err(e) = self.app.attr(
                        &Id::ModelPicker,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::Flag(true),
                    ) {
                        tracing::warn!("Failed to show model picker: {}", e);
                    }
                    self.set_focus(&Id::ModelPicker);
                    self.state.should_redraw = true;
                    None
                }
                Msg::ModelSelected(key) => {
                    // Hide picker and return focus to input box
                    let _ = self.app.attr(
                        &Id::ModelPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    self.set_focus(&Id::InputBox);

                    let coord = Arc::clone(&self.kernel);
                    let tx = self.cmd_tx.clone();
                    let session_id = self.session_id.clone();
                    tokio::spawn(async move {
                        let sid = kernel::types::SessionId::from(session_id);
                        match coord.set_session_model(&sid, &key).await {
                            Ok(()) => {
                                // Resolve display info from local config; fall back
                                // gracefully if the key is unknown locally (e.g.
                                // remote daemon with a different config).
                                let config = crate::config();
                                let (model_id, context_window) =
                                    config.models.iter().find(|m| m.name == key).map_or_else(
                                        || (key.clone(), 0),
                                        |m| (m.model_id.clone(), m.context_window),
                                    );
                                let _ = tx.send(Msg::ModelSwitched {
                                    key,
                                    model_id,
                                    context_window,
                                });
                            }
                            Err(e) => {
                                let _ = tx.send(Msg::Notification(Notification::error(
                                    format!("Failed to switch model: {e}"),
                                    4000,
                                )));
                            }
                        }
                    });
                    None
                }
                Msg::ModelSwitched {
                    key,
                    model_id,
                    context_window,
                } => {
                    self.model_name.clone_from(&model_id);
                    if context_window > 0 {
                        self.context_window = context_window;
                    }
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom(attr::SET_MODEL_NAME),
                        AttrValue::String(model_id.clone()),
                    );
                    let _ = self.app.attr(
                        &Id::Banner,
                        Attribute::Custom(attr::MODEL_NAME),
                        AttrValue::String(model_id),
                    );
                    self.show_notification(&Notification::success(
                        format!("Switched to '{key}' (takes effect next turn)"),
                        3000,
                    ));
                    self.state.should_redraw = true;
                    None
                }
                Msg::CloseModelPicker => {
                    let _ = self.app.attr(
                        &Id::ModelPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    self.set_focus(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CommandRewind => {
                    let coord = Arc::clone(&self.kernel);
                    let tx = self.cmd_tx.clone();
                    let session_id = self.session_id.clone();
                    tokio::spawn(async move {
                        let checkpoints = coord
                            .get_checkpoints(&kernel::types::SessionId::from(session_id))
                            .await
                            .unwrap_or_default();
                        if let Err(e) = tx.send(Msg::CheckpointList(checkpoints)) {
                            tracing::debug!("cmd channel closed, dropping checkpoint list: {e}");
                        }
                    });
                    None
                }
                Msg::CheckpointList(checkpoints) => {
                    if checkpoints.is_empty() {
                        self.show_notification(&Notification::info(
                            "No checkpoints found for this session",
                            3000,
                        ));
                        return None;
                    }

                    let items: Vec<PickerItem> = checkpoints
                        .into_iter()
                        .map(|cp| {
                            let time_str =
                                chrono::DateTime::from_timestamp(cp.created_at as i64, 0)
                                    .map_or_else(
                                        || "?".to_string(),
                                        |dt| dt.format("%H:%M:%S").to_string(),
                                    );
                            let label = format!(
                                "[{}] {} - {} files",
                                time_str, cp.summary, cp.files_changed
                            );
                            PickerItem::new(cp.message_id, label)
                        })
                        .collect();

                    let _ = self.app.attr(
                        &Id::CheckpointPicker,
                        Attribute::Custom(attr::PICKER_ITEMS),
                        AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(items))),
                    );
                    let _ = self.app.attr(
                        &Id::CheckpointPicker,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::Flag(true),
                    );
                    self.set_focus(&Id::CheckpointPicker);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CommandUndo => {
                    let coord = Arc::clone(&self.kernel);
                    let tx = self.cmd_tx.clone();
                    let ctrl_tx = self.ctrl_tx.clone();
                    let session_id = self.session_id.clone();
                    tokio::spawn(async move {
                        let checkpoints = coord
                            .get_checkpoints(&kernel::types::SessionId::from(session_id))
                            .await
                            .unwrap_or_default();

                        if checkpoints.is_empty() {
                            if let Err(e) = tx.send(Msg::Notification(Notification::error(
                                "No checkpoints to undo",
                                3000,
                            ))) {
                                tracing::debug!("cmd channel closed, dropping notification: {e}");
                            }
                            return;
                        }

                        let latest = checkpoints.into_iter().max_by_key(|cp| cp.sequence);
                        if let Some(cp) = latest {
                            let _ = ctrl_tx.try_send(Command::Rewind {
                                message_id: kernel::types::MessageId::from(cp.message_id.clone()),
                                target: kernel::checkpoint::RewindTarget::Both,
                            });
                            if let Err(e) = tx.send(Msg::Notification(Notification::info(
                                format!("Undoing: {}", cp.summary),
                                3000,
                            ))) {
                                tracing::debug!("cmd channel closed, dropping notification: {e}");
                            }
                        }
                    });
                    None
                }
                Msg::CheckpointSelected(message_id, target) => {
                    // Hide picker
                    let _ = self.app.attr(
                        &Id::CheckpointPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    self.set_focus(&Id::InputBox);

                    // Convert msg::RewindTarget to kernel::checkpoint::RewindTarget
                    let kernel_target = match target {
                        crate::msg::RewindTarget::Conversation => {
                            kernel::checkpoint::RewindTarget::Conversation
                        }
                        crate::msg::RewindTarget::Files => kernel::checkpoint::RewindTarget::Files,
                        crate::msg::RewindTarget::Both => kernel::checkpoint::RewindTarget::Both,
                    };

                    // Send rewind command to kernel
                    let _ = self.ctrl_tx.try_send(Command::Rewind {
                        message_id: kernel::types::MessageId::from(message_id),
                        target: kernel_target,
                    });

                    self.show_notification(&Notification::info(
                        format!("Rewinding to checkpoint (target: {target:?})..."),
                        3000,
                    ));
                    self.state.should_redraw = true;
                    None
                }
                Msg::CloseCheckpointPicker => {
                    // Hide checkpoint picker and return focus to input box
                    let _ = self.app.attr(
                        &Id::CheckpointPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    self.set_focus(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CloseHelpDialog => {
                    // Hide help dialog and return focus to input box
                    if let Err(e) = self.app.attr(
                        &Id::HelpDialog,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    ) {
                        tracing::warn!("Failed to hide help dialog: {}", e);
                    }
                    self.set_focus(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                _ => None,
            }
        } else {
            None
        }
    }
}
