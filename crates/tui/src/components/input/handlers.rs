//! Event handlers for the input component

use tuirealm::event::{Event, Key, KeyEvent, KeyModifiers, MouseEvent};

use crate::{
    components::{info_bar::Notification, input_edit::TextInput},
    msg::Msg,
};

use super::component::InputComponent;

impl InputComponent {
    /// Handle all input events - mode-aware handling
    pub(crate) fn handle_input(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        // Browse mode: navigation shortcuts take priority
        if self.mode == crate::app::AppMode::Browse {
            return self.handle_browse_input(ev);
        }

        // Normal mode: text input with some shortcuts
        self.handle_normal_input(ev)
    }

    /// Handle input in browse mode - navigation keys
    fn handle_browse_input(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        match *ev {
            // Browse mode navigation
            Event::Keyboard(KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollDown),
            Event::Keyboard(KeyEvent {
                code: Key::Char('k'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollUp),
            Event::Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::PageHalfUp),
            Event::Keyboard(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::PageHalfDown),
            // ESC or 'q' to exit browse mode
            Event::Keyboard(KeyEvent {
                code: Key::Char('q') | Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ToggleBrowseMode),
            // Go to top/bottom (vim-style)
            Event::Keyboard(KeyEvent {
                code: Key::Char('g'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::GoToTop),
            Event::Keyboard(KeyEvent {
                code: Key::Char('G'),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => Some(Msg::GoToBottom),
            // Toggle expand all with Ctrl+E in browse mode
            Event::Keyboard(KeyEvent {
                code: Key::Char('e'),
                modifiers: KeyModifiers::CONTROL,
            }) => Some(Msg::ToggleExpandAll),
            // Pass through to normal input handler for other keys
            _ => self.handle_normal_input(ev),
        }
    }

    /// Parse slash command from input
    /// Returns Some(Msg) for known commands, None for unknown (treated as regular message)
    fn parse_command(content: &str) -> Option<Msg> {
        if !content.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        match parts[0] {
            "/new" => Some(Msg::CommandNew),
            "/clear" => Some(Msg::CommandClear),
            "/todos" => Some(Msg::CommandTodos),
            "/yolo" => Some(Msg::CommandYolo),
            "/browse" => Some(Msg::CommandBrowse),
            "/sessions" => Some(Msg::CommandSessions),
            "/compact" => Some(Msg::CommandCompact),
            "/help" => Some(Msg::CommandHelp),
            _ => None, // Unknown command: treat as regular message
        }
    }

    /// Handle input in normal mode - text editing
    fn handle_normal_input(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        // File completion mode - handle special keys first (use is_active, not is_visible)
        if self.file_completion.is_active() {
            return Some(self.handle_file_completion_input(ev));
        }

        // Command completion mode - handle special keys
        if self.command_completion.is_visible() {
            return Some(self.handle_command_completion_input(ev));
        }

        // Handle paste event first (needs to borrow text)
        if let Event::Paste(text) = ev {
            return Some(self.handle_text_paste(text));
        }

        // Handle mouse events for text selection
        if let Event::Mouse(MouseEvent {
            kind, column, row, ..
        }) = ev
        {
            let result = self.component.handle_mouse_event(*kind, *column, *row);

            match result {
                super::editor::MouseEventResult::NotHandled => {}
                super::editor::MouseEventResult::Handled => {
                    // If selection was copied, show status message
                    if matches!(kind, tuirealm::event::MouseEventKind::Up(_)) {
                        if let Some(text) = self.component.get_selected_text() {
                            return Some(Msg::Notification(Notification::unknown(
                                format!("📋 {text}"),
                                2000,
                            )));
                        }
                    }
                    return Some(Msg::Redraw);
                }
                super::editor::MouseEventResult::HandledWithScroll => {
                    // Auto-scroll during drag - return Redraw to continue scrolling
                    return Some(Msg::Redraw);
                }
            }
        }

        match *ev {
            // Ctrl+V: paste from clipboard (fallback for systems without bracketed paste)
            Event::Keyboard(KeyEvent {
                code: Key::Char('v'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                // If there's a selection, delete it first
                if self.component.has_selection() {
                    self.component.delete_selection();
                }
                // Try to paste image first
                if let Some(placeholder) = self.try_paste_image() {
                    self.component.insert_str(&placeholder);
                    self.update_completion();
                    return Some(Msg::InputChanged(self.component.content().to_string()));
                }
                // Fall back to reading text from clipboard
                #[cfg(not(target_os = "macos"))]
                {
                    use arboard::Clipboard;
                    match Clipboard::new() {
                        Ok(mut clipboard) => match clipboard.get_text() {
                            Ok(text) => return Some(self.handle_text_paste(&text)),
                            Err(e) => tracing::debug!("No text in clipboard: {}", e),
                        },
                        Err(e) => tracing::debug!("Failed to create clipboard: {}", e),
                    }
                }
                None
            }
            // @: start file completion (must be before generic Char handler)
            Event::Keyboard(KeyEvent {
                code: Key::Char('@'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.insert_char('@');
                self.start_file_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                // If there's a selection, delete it first, then insert the character
                if self.component.has_selection() {
                    self.component.delete_selection();
                }
                self.component.insert_char(c);
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            // Shift+Enter or Ctrl+J: insert newline
            Event::Keyboard(
                KeyEvent {
                    code: Key::Enter,
                    modifiers: KeyModifiers::SHIFT,
                }
                | KeyEvent {
                    code: Key::Char('j'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component.insert_newline();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            // Enter: submit input
            Event::Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => {
                // If completion is visible, accept it (same as Tab)
                if self.command_completion.is_visible() {
                    self.accept_completion();
                    self.update_completion();
                    return Some(Msg::InputChanged(self.component.content().to_string()));
                }
                // Get content blocks (supports multi-modal: text, images, etc.)
                let content_blocks = self.get_content_blocks();
                // Check if content is effectively empty (no text and no images)
                let has_content = content_blocks.iter().any(|block| match block {
                    kernel::types::ContentBlock::Text { text } => !text.trim().is_empty(),
                    _ => true,
                });
                if has_content {
                    // Check if it's a command (only supports text-only content)
                    let text_content = self.component.content();
                    if let Some(cmd_msg) = Self::parse_command(text_content) {
                        // It's a command, return the command message
                        // Clear input after submitting command
                        let _ = self.component.submit();
                        Some(cmd_msg)
                    } else {
                        // Regular input with multi-modal support
                        // Clear input and mappings after submitting
                        let _ = self.component.submit();
                        self.placeholder_counter = 0;
                        self.image_paths.clear();
                        self.pasted_contents.clear();
                        Some(Msg::InputSubmit(content_blocks))
                    }
                } else {
                    None
                }
            }
            Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                // If there's a selection, delete it; otherwise do normal backspace
                if self.component.has_selection() {
                    self.component.delete_selection();
                } else {
                    self.component.backspace();
                }
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            Event::Keyboard(KeyEvent {
                code: Key::Delete,
                modifiers: KeyModifiers::NONE,
            }) => {
                // If there's a selection, delete it; otherwise do normal delete
                if self.component.has_selection() {
                    self.component.delete_selection();
                } else {
                    self.component.delete_char();
                }
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            Event::Keyboard(KeyEvent {
                code: Key::Left,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.move_and_clear_selection(|c| c.move_left());
                None
            }
            Event::Keyboard(KeyEvent {
                code: Key::Right,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.move_and_clear_selection(|c| c.move_right());
                None
            }
            // Home or Ctrl+A: move to start of line
            Event::Keyboard(
                KeyEvent {
                    code: Key::Home,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('a'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component
                    .move_and_clear_selection(|c| c.move_to_start_of_line());
                None
            }
            // End or Ctrl+E: move to end of line
            Event::Keyboard(
                KeyEvent {
                    code: Key::End,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('e'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component
                    .move_and_clear_selection(|c| c.move_to_end_of_line());
                None
            }
            // Alt+B: move backward one word
            Event::Keyboard(KeyEvent {
                code: Key::Char('b'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component
                    .move_and_clear_selection(|c| c.move_word_left());
                None
            }
            // Alt+F: move forward one word
            Event::Keyboard(KeyEvent {
                code: Key::Char('f'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component
                    .move_and_clear_selection(|c| c.move_word_right());
                None
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.kill_to_start_of_line();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('w'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.delete_word_backward();
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            // Tab: accept completion or insert spaces
            Event::Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                if self.command_completion.is_visible() {
                    self.accept_completion();
                    self.update_completion();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                } else {
                    // Insert tab/indent when no completion
                    self.component.insert_str("    ");
                    Some(Msg::InputChanged(self.component.content().to_string()))
                }
            }
            // Up arrow: navigate completion or history
            Event::Keyboard(KeyEvent {
                code: Key::Up,
                modifiers: KeyModifiers::NONE,
            }) => {
                if self.command_completion.is_visible() {
                    self.completion_prev();
                    Some(Msg::Redraw)
                } else if self.file_completion.is_active() {
                    self.file_completion_prev();
                    Some(Msg::Redraw)
                } else if self.component.is_on_first_line() {
                    self.history_prev();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                } else {
                    self.component.move_and_clear_selection(|c| c.move_up());
                    None
                }
            }
            // Down arrow: navigate completion or history
            Event::Keyboard(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::NONE,
            }) => {
                if self.command_completion.is_visible() {
                    self.completion_next();
                    Some(Msg::Redraw)
                } else if self.file_completion.is_active() {
                    self.file_completion_next();
                    Some(Msg::Redraw)
                } else if self.component.is_on_last_line() {
                    self.history_next();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                } else {
                    self.component.move_and_clear_selection(|c| c.move_down());
                    None
                }
            }
            // Ctrl+P: navigate completion or history
            Event::Keyboard(KeyEvent {
                code: Key::Char('p'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                if self.command_completion.is_visible() {
                    self.completion_prev();
                    Some(Msg::Redraw)
                } else if self.file_completion.is_active() {
                    self.file_completion_prev();
                    Some(Msg::Redraw)
                } else {
                    self.history_prev();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                }
            }
            // Ctrl+N: navigate completion or history
            Event::Keyboard(KeyEvent {
                code: Key::Char('n'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                if self.command_completion.is_visible() {
                    self.completion_next();
                    Some(Msg::Redraw)
                } else if self.file_completion.is_active() {
                    self.file_completion_next();
                    Some(Msg::Redraw)
                } else {
                    self.history_next();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                }
            }
            Event::Keyboard(KeyEvent {
                code: Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::CancelRequest),
            Event::Keyboard(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                if self.component.handle_ctrl_c() {
                    Some(Msg::Quit)
                } else {
                    // First Ctrl+C: show hint in status bar for 1 second
                    Some(Msg::Notification(Notification::warn(
                        "Press Ctrl+C again to exit",
                        1000, // 1000ms = 1 second, matches double-press detection
                    )))
                }
            }
            // Note: PageUp/PageDown and mouse scroll events are handled by ChatViewComponent
            // Toggle browse mode with Ctrl+O
            Event::Keyboard(KeyEvent {
                code: Key::Char('o'),
                modifiers: KeyModifiers::CONTROL,
            }) => Some(Msg::ToggleBrowseMode),
            // History search with Ctrl+R (telescope-style fuzzy finder)
            Event::Keyboard(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::CONTROL,
            }) => Some(Msg::ShowHistoryPicker),
            // Suspend process to background with Ctrl+Z
            Event::Keyboard(KeyEvent {
                code: Key::Char('z'),
                modifiers: KeyModifiers::CONTROL,
            }) => Some(Msg::Suspend),
            _ => None,
        }
    }
}
