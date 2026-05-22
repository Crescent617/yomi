//! Update method for handling messages

use tuirealm::{
    props::{AttrValue, Attribute},
    terminal::TerminalAdapter,
};

use std::sync::Arc;

use crate::{
    attr,
    components::{default_help_sections, info_bar::Notification, PickerItem},
    id::Id,
    msg::Msg,
};
use kernel::event::ControlCommand;
use kernel::permissions::Level;

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
                    let _ = self.ctrl_tx.try_send(ControlCommand::Cancel);
                    None
                }
                Msg::ClearQueuedMessage => {
                    self.clear_queued_message();
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
                    // Send permission response based on selection
                    // idx: 0 = Approve, 1 = Always approve, 2 = Deny, 3 = YOLO
                    if let Some(req_id) = self.pending_permission.take() {
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
                                let _ = self
                                    .ctrl_tx
                                    .try_send(ControlCommand::SetLevel(Level::Dangerous));
                                (true, false)
                            }
                            _ => (false, false), // Deny
                        };
                        let _ = self.ctrl_tx.try_send(ControlCommand::Response {
                            req_id,
                            approved,
                            remember,
                        });
                    }
                    // Return focus to input box
                    self.set_focus(&Id::InputBox);
                    None
                }
                Msg::DialogCancelled => {
                    // Deny the permission request if dialog is cancelled
                    if let Some(req_id) = self.pending_permission.take() {
                        let _ = self.ctrl_tx.try_send(ControlCommand::Response {
                            req_id,
                            approved: false,
                            remember: false,
                        });
                    }
                    // Return focus to input box
                    self.set_focus(&Id::InputBox);
                    None
                }
                // Slash commands
                Msg::CommandNew => {
                    // Signal that a new session should be created
                    self.state.should_create_new_session = true;
                    self.state.quit = true;
                    None
                }
                Msg::CommandGoal(description) => {
                    let state = kernel::goal::GoalState::new(description);
                    let _ = self.ctrl_tx.try_send(ControlCommand::StartGoal(state));
                    self.show_notification(&Notification::info(
                        "Goal mode activated. Agent will work autonomously. Use /goal:stop to interrupt.",
                        5000,
                    ));
                    None
                }
                Msg::CommandGoalStop => {
                    let _ = self.ctrl_tx.try_send(ControlCommand::StopGoal);
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
                    let _ = self.ctrl_tx.try_send(ControlCommand::SetLevel(new_level));

                    None
                }
                Msg::CommandBrowse => {
                    // Toggle browse mode
                    Box::pin(self.update(Some(Msg::ToggleBrowseMode))).await
                }
                Msg::CommandCompact => {
                    // Send compact request
                    let _ = self.ctrl_tx.try_send(ControlCommand::Compact);
                    self.show_notification(&Notification::info("Compacting messages...", 3000));
                    None
                }
                Msg::CommandReload => {
                    // Reload skills and hooks from disk via daemon
                    let coord = Arc::clone(&self.coordinator);
                    match coord.reload_agent_config().await {
                        Ok(()) => {
                            self.show_notification(&Notification::info("Reloaded", 3000));
                        }
                        Err(e) => {
                            self.show_notification(&Notification::error(
                                format!("Reload failed: {e}"),
                                5000,
                            ));
                        }
                    }
                    None
                }
                Msg::Suspend => {
                    // Suspend process to background (Ctrl-Z)
                    self.suspend_process();
                    None
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
                    // Load sessions for current working dir and show picker
                    let working_dir = self.working_dir.to_string_lossy().to_string();
                    let args = kernel::storage::session::ListArgs {
                        working_dir: Some(working_dir),
                        limit: Some(50),
                        ..Default::default()
                    };
                    let sessions = self
                        .coordinator
                        .list_sessions_filtered(args)
                        .await
                        .unwrap_or_default();

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

                    // Show the session picker
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
                    // Give focus to session picker
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
                Msg::CommandRewind => {
                    // Load checkpoints for current session and show picker
                    let session_id = self.session_id.clone();
                    let checkpoints = self
                        .coordinator
                        .get_checkpoints(&kernel::types::SessionId(session_id))
                        .await
                        .unwrap_or_default();

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

                    // Show the checkpoint picker (like CommandSessions does)
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
                    // Give focus to checkpoint picker
                    self.set_focus(&Id::CheckpointPicker);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CommandUndo => {
                    // Undo last turn: rewind to the latest checkpoint
                    let session_id = self.session_id.clone();
                    let checkpoints = self
                        .coordinator
                        .get_checkpoints(&kernel::types::SessionId(session_id))
                        .await
                        .unwrap_or_default();

                    if checkpoints.is_empty() {
                        self.show_notification(&Notification::error(
                            "No checkpoints to undo",
                            3000,
                        ));
                        return None;
                    }

                    // Find the latest checkpoint (highest sequence)
                    let latest = checkpoints.into_iter().max_by_key(|cp| cp.sequence);

                    if let Some(cp) = latest {
                        // Send rewind command to coordinator (Both = conversation + files)
                        let _ = self.ctrl_tx.try_send(ControlCommand::Rewind {
                            message_id: kernel::types::MessageId::from_string(cp.message_id),
                            target: kernel::checkpoint::RewindTarget::Both,
                        });
                        self.show_notification(&Notification::info(
                            format!("Undoing: {}", cp.summary),
                            3000,
                        ));
                    }
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

                    // Send rewind command to coordinator
                    let _ = self.ctrl_tx.try_send(ControlCommand::Rewind {
                        message_id: kernel::types::MessageId::from_string(message_id),
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
