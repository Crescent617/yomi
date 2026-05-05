//! UI message/command handling

use tuirealm::props::{AttrValue, Attribute};
use tuirealm::terminal::TerminalAdapter;

use kernel::event::ControlCommand;
use kernel::permissions::Level;
use kernel::types::ContentBlock;

use crate::app::init::ComponentInit;
use crate::app::state::AppMode;
use crate::app::streaming::QueuedMessageOps;
use crate::app::types::Model;
use crate::app::ui_ops::UiOps;
use crate::attr;
use crate::components::{PickerItem, default_help_sections};
use crate::components::status_bar::Tip;
use crate::id::Id;
use crate::msg::Msg;
use crate::utils::text::{substring_by_chars, truncate_by_chars};

/// Command handling trait (previously `update`)
pub trait CommandHandler {
    fn handle_command(&mut self, msg: Option<Msg>) -> Option<Msg>;
}

impl CommandHandler for Model {
    fn handle_command(&mut self, msg: Option<Msg>) -> Option<Msg> {
        if let Some(msg) = msg {
            self.state.should_redraw = true;

            match msg {
                Msg::Quit => {
                    self.state.quit = true;
                    None
                }
                Msg::InputSubmit(blocks) => self.handle_input_submit(blocks),
                Msg::ScrollUp => {
                    let amount = if self.mode() == AppMode::Browse { 1 } else { 3 };
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_UP),
                        AttrValue::Number(amount as isize),
                    );
                    None
                }
                Msg::ScrollDown => {
                    let amount = if self.mode() == AppMode::Browse { 1 } else { 3 };
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_DOWN),
                        AttrValue::Number(amount as isize),
                    );
                    None
                }
                Msg::InputChanged(_) => {
                    if self.mode() == AppMode::Browse {
                        return None;
                    }
                    None
                }
                Msg::CancelRequest => {
                    let _ = self.ctrl_tx.try_send(ControlCommand::Cancel);
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
                Msg::ToggleBrowseMode => {
                    self.toggle_browse_mode();
                    None
                }
                Msg::PageHalfUp => self.handle_page_half_up(),
                Msg::PageHalfDown => self.handle_page_half_down(),
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
                Msg::DialogSelected(idx) => self.handle_dialog_selected(idx),
                Msg::DialogCancelled => self.handle_dialog_cancelled(),
                Msg::CommandNew => {
                    self.state.should_create_new_session = true;
                    self.state.quit = true;
                    None
                }
                Msg::CommandClear => self.handle_command_clear(),
                Msg::CommandTodos => {
                    let _ = self.app.attr(
                        &Id::TodoList,
                        Attribute::Custom(attr::TOGGLE_TODOS),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::CommandYolo => self.handle_command(Some(Msg::ToggleYoloMode)),
                Msg::ToggleYoloMode => {
                    self.toggle_yolo_mode();
                    None
                }
                Msg::CommandBrowse => self.handle_command(Some(Msg::ToggleBrowseMode)),
                Msg::CommandCompact => {
                    let _ = self.ctrl_tx.try_send(ControlCommand::Compact);
                    self.show_notification(&crate::components::info_bar::Notification::info(
                        "Compacting messages...",
                        3000,
                    ));
                    None
                }
                Msg::Suspend => {
                    self.suspend_process();
                    None
                }
                Msg::ShowHistoryPicker => self.handle_show_history_picker(),
                Msg::HistorySelected(idx_str) => self.handle_history_selected(&idx_str),
                Msg::CloseHistoryPicker => {
                    let _ = self.app.active(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CommandHelp => self.handle_command_help(),
                Msg::CommandSessions => self.handle_command_sessions(),
                Msg::SessionSelected(session_id) => {
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
                    let _ = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    let _ = self.app.active(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CloseHelpDialog => {
                    let _ = self.app.attr(
                        &Id::HelpDialog,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    let _ = self.app.active(&Id::InputBox);
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

/// Individual command handlers
trait CommandHandlers {
    fn handle_input_submit(&mut self, blocks: Vec<ContentBlock>) -> Option<Msg>;
    fn handle_dialog_selected(&mut self, idx: usize) -> Option<Msg>;
    fn handle_dialog_cancelled(&mut self) -> Option<Msg>;
    fn handle_page_half_up(&mut self) -> Option<Msg>;
    fn handle_page_half_down(&mut self) -> Option<Msg>;
    fn handle_command_clear(&mut self) -> Option<Msg>;
    fn handle_show_history_picker(&mut self) -> Option<Msg>;
    fn handle_history_selected(&mut self, idx_str: &str) -> Option<Msg>;
    fn handle_command_help(&mut self) -> Option<Msg>;
    fn handle_command_sessions(&mut self) -> Option<Msg>;
}

impl CommandHandlers for Model {
    fn handle_input_submit(&mut self, blocks: Vec<ContentBlock>) -> Option<Msg> {
        if self.mode() == AppMode::Browse {
            return None;
        }

        // Extract text content for history navigation
        let text_content: String = blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Save to history for C-n/C-p navigation
        if !text_content.trim().is_empty() {
            self.input_history.retain(|h| h != &text_content);
            self.input_history.push(text_content.clone());
            let _ = self.init_input_history();
        }

        // Call input hook if provided
        if let Some(ref hook) = self.on_input_hook {
            hook(&self.session_id);
        }

        // Check if we're currently streaming
        if self.state.is_streaming {
            self.set_queued_message(blocks);
        } else {
            let _ = self.input_tx.try_send(blocks);
        }
        None
    }

    fn handle_dialog_selected(&mut self, idx: usize) -> Option<Msg> {
        if let Some(req_id) = self.pending_permission.take() {
            let (approved, remember) = match idx {
                0 => (true, false),
                1 => (true, true),
                3 => {
                    self.permission_level = Level::Dangerous;
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom(attr::SET_PERMISSION_LEVEL),
                        AttrValue::Number(2),
                    );
                    self.show_notification(&crate::components::info_bar::Notification::info(
                        "YOLO mode enabled - all tools will be auto-approved",
                        5000,
                    ));
                    let _ = self.ctrl_tx.try_send(ControlCommand::SetLevel(Level::Dangerous));
                    (true, false)
                }
                _ => (false, false),
            };
            let _ = self.ctrl_tx.try_send(ControlCommand::Response {
                req_id,
                approved,
                remember,
            });
        }
        let _ = self.app.active(&Id::InputBox);
        None
    }

    fn handle_dialog_cancelled(&mut self) -> Option<Msg> {
        if let Some(req_id) = self.pending_permission.take() {
            let _ = self.ctrl_tx.try_send(ControlCommand::Response {
                req_id,
                approved: false,
                remember: false,
            });
        }
        let _ = self.app.active(&Id::InputBox);
        None
    }

    fn handle_page_half_up(&mut self) -> Option<Msg> {
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

    fn handle_page_half_down(&mut self) -> Option<Msg> {
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

    fn handle_command_clear(&mut self) -> Option<Msg> {
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::CLEAR_HISTORY),
            AttrValue::Flag(true),
        );
        let _ = self.app.attr(
            &Id::TodoList,
            Attribute::Custom(attr::CLEAR_TODOS),
            AttrValue::Flag(true),
        );
        None
    }

    fn handle_show_history_picker(&mut self) -> Option<Msg> {
        let items = self.history_items();
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
        let _ = self.app.active(&Id::HistoryPicker);
        None
    }

    fn handle_history_selected(&mut self, idx_str: &str) -> Option<Msg> {
        if let Some(idx_part) = idx_str.strip_prefix("history_") {
            if let Ok(idx) = idx_part.parse::<usize>() {
                if idx < self.input_history.len() {
                    let selected_text = self.input_history[idx].clone();
                    let _ = self.app.attr(
                        &Id::InputBox,
                        Attribute::Custom(attr::INPUT_CONTENT),
                        AttrValue::String(selected_text),
                    );
                }
            }
        }
        let _ = self.app.active(&Id::InputBox);
        self.state.should_redraw = true;
        None
    }

    fn handle_command_help(&mut self) -> Option<Msg> {
        let sections = default_help_sections();
        if let Err(e) = self.app.attr(
            &Id::HelpDialog,
            Attribute::Custom(attr::DIALOG_SHOW),
            AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(sections))),
        ) {
            tracing::warn!("Failed to show help dialog: {}", e);
        }
        if let Err(e) = self.app.active(&Id::HelpDialog) {
            tracing::warn!("Failed to focus help dialog: {}", e);
        }
        self.state.should_redraw = true;
        None
    }

    fn handle_command_sessions(&mut self) -> Option<Msg> {
        let working_dir = self.working_dir.to_string_lossy().to_string();
        let args = kernel::ListArgs {
            working_dir: Some(working_dir),
            limit: Some(50),
            ..Default::default()
        };
        let sessions = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.session_store.list(args))
        })
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
                let short_id = format_short_id(&id_str);
                let label = format!("{short_id} - {age_str}");
                PickerItem::new(id_str, label).with_meta(preview)
            })
            .collect();

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
        if let Err(e) = self.app.active(&Id::SessionPicker) {
            tracing::warn!("Failed to focus session picker: {}", e);
        }
        self.state.should_redraw = true;
        None
    }
}

/// Mode and utility operations
trait ModeOps {
    fn mode(&self) -> AppMode;
    #[allow(dead_code)]
    fn set_mode(&mut self, mode: AppMode);
    fn toggle_browse_mode(&mut self);
    fn toggle_yolo_mode(&mut self);
    fn history_items(&self) -> Vec<PickerItem>;
}

impl ModeOps for Model {
    fn mode(&self) -> AppMode {
        // Get mode from app state - stored in StatusBar or derive from state
        // For now, we track it in the Model struct
        // This is a placeholder - we'll need to store mode in Model
        AppMode::Normal // Default
    }

    fn set_mode(&mut self, _mode: AppMode) {
        // Placeholder - mode will be stored in Model
    }

    fn toggle_browse_mode(&mut self) {
        // Read current mode from StatusBar
        let current_mode = self.mode();

        match current_mode {
            AppMode::Normal => {
                // Enter browse mode
                let _ = self.app.attr(
                    &Id::StatusBar,
                    Attribute::Custom(attr::SET_MODE),
                    AttrValue::Number(1),
                );
                let _ = self.app.attr(
                    &Id::InputBox,
                    Attribute::Custom(attr::MODE),
                    AttrValue::Number(1),
                );
                let _ = self.app.attr(
                    &Id::StatusBar,
                    Attribute::Custom(attr::SHOW_TIP),
                    Tip::new("C-o toggle, C-e expand, j/k/g/G scroll, q exit", 0).to_attr_value(),
                );
            }
            AppMode::Browse => {
                // Exit browse mode
                let _ = self.app.attr(
                    &Id::ChatView,
                    Attribute::Custom(attr::COLLAPSE_ALL),
                    AttrValue::Flag(true),
                );
                let _ = self.app.attr(
                    &Id::StatusBar,
                    Attribute::Custom(attr::SET_MODE),
                    AttrValue::Number(0),
                );
                let _ = self.app.attr(
                    &Id::InputBox,
                    Attribute::Custom(attr::MODE),
                    AttrValue::Number(0),
                );
                let _ = self.app.attr(
                    &Id::StatusBar,
                    Attribute::Custom(attr::CLEAR_TIP),
                    AttrValue::Flag(true),
                );
                let _ = self.app.attr(
                    &Id::StatusBar,
                    Attribute::Custom(attr::CLEAR_SCROLL_PROGRESS),
                    AttrValue::Flag(true),
                );
            }
        }
    }

    fn toggle_yolo_mode(&mut self) {
        let new_level = if self.permission_level == Level::Dangerous {
            Level::Safe
        } else {
            Level::Dangerous
        };
        self.permission_level = new_level;

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

        let msg = if new_level == Level::Dangerous {
            " YOLO mode enabled - all tools will be auto-approved"
        } else {
            " YOLO mode disabled"
        };
        self.show_notification(&crate::components::info_bar::Notification::info(msg, 5000));
        let _ = self.ctrl_tx.try_send(ControlCommand::SetLevel(new_level));
    }

    fn history_items(&self) -> Vec<PickerItem> {
        self.input_history
            .iter()
            .enumerate()
            .map(|(idx, text)| {
                let text_single_line = text.replace('\n', " ").trim_start().to_string();
                PickerItem::new(
                    format!("history_{idx}"),
                    truncate_by_chars(&text_single_line, 50),
                )
            })
            .rev()
            .collect()
    }
}

/// Format a session ID for display, truncating long IDs with ellipsis.
fn format_short_id(id: &str) -> String {
    let char_count = id.chars().count();
    if char_count > 12 {
        let start = substring_by_chars(id, 0, 6);
        let end = substring_by_chars(id, char_count.saturating_sub(4), char_count);
        format!("{start}...{end}")
    } else {
        id.to_string()
    }
}
