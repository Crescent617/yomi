//! Command and file completion logic for the input component

use tuirealm::{
    event::Event,
    ratatui::{layout::Rect, text::Line, Frame},
};

use crate::{
    components::{input_edit::TextInput, CompletionList},
    msg::Msg,
};

use super::component::InputComponent;

impl InputComponent {
    /// Generic helper to render a completion list dropdown
    pub(crate) fn render_completion_dropdown<T>(
        list: &mut CompletionList<T>,
        frame: &mut Frame,
        area: Rect,
        max_visible: usize,
        footer_lines: u16,
        render_item: impl Fn(&T, usize, usize) -> Line,
    ) {
        // Note: callers check if completion is active before calling this function
        // We only check if the list has items to render
        if list.is_empty() {
            return;
        }

        // Ensure selected item is visible (sticky window behavior)
        list.ensure_visible(max_visible);
        let scroll_offset = list.scroll_offset();

        let visible_count = list.len().min(max_visible);
        let height = visible_count as u16 + footer_lines;
        let dropdown_area = Rect {
            x: area.x,
            y: area.y.saturating_sub(height),
            width: area.width,
            height,
        };

        // Clear the area first
        frame.render_widget(tuirealm::ratatui::widgets::Clear, dropdown_area);

        // Render items with scrolling
        let items: Vec<Line> = list
            .items()
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(max_visible)
            .map(|(i, item)| render_item(item, i, list.selected_index()))
            .collect();

        let widget =
            tuirealm::ratatui::widgets::Paragraph::new(tuirealm::ratatui::text::Text::from(items));
        frame.render_widget(widget, dropdown_area);
    }

    /// Start command completion at the given cursor position
    pub(crate) fn start_command_completion(&mut self, cursor_pos: usize) {
        self.command_query.clear();
        self.command_start_pos = cursor_pos;
        self.refresh_command_list();
    }

    /// Refresh command list based on current query
    pub(crate) fn refresh_command_list(&mut self) {
        let query = &self.command_query;
        let filtered: Vec<(String, String)> = super::SLASH_COMMANDS
            .iter()
            .filter(|(cmd, _)| {
                if query.is_empty() {
                    true
                } else {
                    cmd.to_lowercase().contains(&query.to_lowercase())
                }
            })
            .map(|(cmd, desc)| ((*cmd).to_string(), (*desc).to_string()))
            .collect();
        self.command_completion.show(filtered);
    }

    /// Update command completion state based on current input
    pub(crate) fn update_completion(&mut self) {
        let content = self.component.content();

        // Command names only contain alphanumeric, '_' or '-'
        let should_show = content.starts_with('/')
            && content
                .chars()
                .skip(1)
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ':');

        if should_show && !self.command_completion.is_visible() {
            self.start_command_completion(1);
        } else if !should_show {
            self.command_completion.hide();
            self.command_query.clear();
        }
    }

    /// Select next completion item
    pub(crate) fn completion_next(&mut self) {
        self.command_completion.next();
    }

    /// Select previous completion item
    pub(crate) fn completion_prev(&mut self) {
        self.command_completion.prev();
    }

    /// Accept the selected completion
    pub(crate) fn accept_completion(&mut self) {
        if let Some((cmd, _)) = self.command_completion.get_selected() {
            // Delete the entire query including the leading '/'
            // (command_start_pos is position after '/', so we go back one more)
            let end = self.component.cursor_pos();
            let start = self.command_start_pos.saturating_sub(1);
            for _ in 0..(end - start) {
                self.component.backspace();
            }
            // Insert the selected command followed by a space
            self.component.insert_str(cmd);
            self.component.insert_char(' ');
            self.command_completion.hide();
            self.command_query.clear();
        }
    }

    /// Start file completion (@-mention)
    pub(crate) fn start_file_completion(&mut self) {
        let cursor_pos = self.component.cursor_pos();
        self.file_completion.start(cursor_pos);
    }

    /// Select next file completion item
    pub(crate) fn file_completion_next(&mut self) {
        self.file_completion.next();
    }

    /// Select previous file completion item
    pub(crate) fn file_completion_prev(&mut self) {
        self.file_completion.prev();
    }

    /// Accept the selected file completion.
    /// `add_space` controls whether a trailing space is appended.
    /// For progressive completion in root dir mode, pressing Enter/Tab on a
    /// directory will navigate into it instead of completing.
    pub(crate) fn accept_file_completion(&mut self, add_space: bool) {
        let is_root_dir = self.file_completion.mode()
            == crate::components::file_completion::CompletionMode::RootDir;
        let is_dir = self.file_completion.is_selected_dir();

        // Progressive completion: in root dir mode when pressing Tab on a directory,
        // navigate into it instead of completing.
        if is_root_dir && is_dir && !add_space {
            let _ = self.file_completion.accept_and_continue();
            // Get the new query which includes the selected directory
            let new_query = self.file_completion.query().to_string();

            // Update the input: delete old content and insert new query
            let cursor_pos = self.component.cursor_pos();
            let query_start = self.file_completion.query_start_pos();
            let chars_to_delete = {
                let content = self.component.content();
                content[query_start..cursor_pos].chars().count()
            };
            for _ in 0..chars_to_delete {
                self.component.backspace();
            }
            self.component.insert_str(&new_query);
            // Stay in completion mode, don't hide
            return;
        }

        let selected = match self.file_completion.selected_full_path() {
            Some(s) => s,
            None => return,
        };

        // Delete the query part (from @ to current cursor position, by character count)
        let start = self.file_completion.query_start_pos();
        let end = self.component.cursor_pos();
        let chars_to_delete = {
            let content = self.component.content();
            content[start..end].chars().count()
        };
        for _ in 0..chars_to_delete {
            self.component.backspace();
        }

        // Insert the selected file path
        self.component.insert_str(&selected);

        if add_space {
            self.component.insert_char(' ');
            // Close completion on Enter
            self.file_completion.accept();
        } else {
            // Keep completion open for continued searching on Tab
            self.file_completion.reset_for_continue();
            self.file_completion.refresh_list();
        }
    }

    /// Cancel file completion
    pub(crate) fn cancel_file_completion(&mut self) {
        self.file_completion.cancel();
    }

    /// Cancel command completion
    pub(crate) fn cancel_command_completion(&mut self) {
        self.command_completion.hide();
        self.command_query.clear();
    }

    /// Handle input when command completion is active
    pub(crate) fn handle_command_completion_input(
        &mut self,
        ev: &Event<crate::msg::UserEvent>,
    ) -> Msg {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers};

        match ev {
            // Enter or Tab: accept completion
            Event::Keyboard(KeyEvent {
                code: Key::Enter | Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.accept_completion();
                Msg::InputChanged(self.component.content().to_string())
            }
            // Shift+Tab, Up arrow or Ctrl+P: navigate up
            Event::Keyboard(
                KeyEvent {
                    code: Key::BackTab,
                    modifiers: KeyModifiers::SHIFT,
                }
                | KeyEvent {
                    code: Key::Up,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('p'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.completion_prev();
                Msg::Redraw
            }
            // Escape or Ctrl+C: cancel completion
            Event::Keyboard(
                KeyEvent {
                    code: Key::Esc,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.cancel_command_completion();
                // Also clear the input when Ctrl+C is pressed during completion
                if matches!(
                    ev,
                    Event::Keyboard(KeyEvent {
                        code: Key::Char('c'),
                        modifiers: KeyModifiers::CONTROL,
                    })
                ) {
                    self.component.clear();
                }
                Msg::Redraw
            }
            // Down arrow or Ctrl+N: navigate down
            Event::Keyboard(
                KeyEvent {
                    code: Key::Down,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('n'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.completion_next();
                Msg::Redraw
            }
            // Space: cancel completion and insert space
            Event::Keyboard(KeyEvent {
                code: Key::Char(' '),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.cancel_command_completion();
                self.component.insert_char(' ');
                Msg::InputChanged(self.component.content().to_string())
            }
            // Regular character: add to query and refresh
            Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                self.component.insert_char(*c);
                self.command_query.push(*c);
                self.refresh_command_list();
                Msg::InputChanged(self.component.content().to_string())
            }
            // Backspace: remove from query and refresh
            Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                let cursor_pos = self.component.cursor_pos();
                // Cancel completion if cursor moved before / symbol
                if cursor_pos < self.command_start_pos {
                    self.cancel_command_completion();
                } else {
                    self.command_query.pop();
                    self.refresh_command_list();
                }
                Msg::InputChanged(self.component.content().to_string())
            }
            // Readline shortcuts: handle directly to avoid recursion
            Event::Keyboard(KeyEvent {
                code: Key::Char('w'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.delete_word_backward();
                self.update_command_completion_after_edit();
                Msg::InputChanged(self.component.content().to_string())
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.kill_to_start_of_line();
                self.update_command_completion_after_edit();
                Msg::InputChanged(self.component.content().to_string())
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('a'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component
                    .move_and_clear_selection(|c| c.move_to_start_of_line());
                // Cancel completion if cursor moved before /
                if self.component.cursor_pos() < self.command_start_pos {
                    self.cancel_command_completion();
                }
                Msg::Redraw
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('e'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component
                    .move_and_clear_selection(|c| c.move_to_end_of_line());
                Msg::Redraw
            }
            _ => Msg::Redraw,
        }
    }

    /// Update command completion state after edit
    fn update_command_completion_after_edit(&mut self) {
        let cursor_pos = self.component.cursor_pos();
        if cursor_pos < self.command_start_pos {
            self.cancel_command_completion();
        } else {
            self.command_query.clear();
            if cursor_pos > self.command_start_pos {
                self.command_query =
                    self.component.content()[self.command_start_pos..cursor_pos].to_string();
            }
            self.refresh_command_list();
        }
    }

    /// Handle input when file completion is active
    pub(crate) fn handle_file_completion_input(
        &mut self,
        ev: &Event<crate::msg::UserEvent>,
    ) -> Msg {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers};

        match *ev {
            // Enter: accept completion and append a space
            Event::Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.accept_file_completion(true);
                Msg::InputChanged(self.component.content().to_string())
            }
            // Tab: accept completion without appending a space
            Event::Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.accept_file_completion(false);
                Msg::InputChanged(self.component.content().to_string())
            }
            // Shift+Tab, Up arrow, or Ctrl+P: navigate up
            Event::Keyboard(
                KeyEvent {
                    code: Key::BackTab,
                    modifiers: KeyModifiers::SHIFT,
                }
                | KeyEvent {
                    code: Key::Up,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('p'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.file_completion_prev();
                Msg::Redraw
            }
            // Escape: cancel completion
            Event::Keyboard(KeyEvent {
                code: Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.cancel_file_completion();
                Msg::Redraw
            }
            // Ctrl+C: cancel completion and clear input
            Event::Keyboard(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.cancel_file_completion();
                self.component.clear();
                Msg::Redraw
            }
            // Down arrow or Ctrl+N: navigate down
            Event::Keyboard(
                KeyEvent {
                    code: Key::Down,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('n'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.file_completion_next();
                Msg::Redraw
            }
            // Space: cancel completion and insert space
            Event::Keyboard(KeyEvent {
                code: Key::Char(' '),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.cancel_file_completion();
                self.component.insert_char(' ');
                Msg::InputChanged(self.component.content().to_string())
            }
            // Regular character: let FileCompletion handle it
            Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                self.component.insert_char(c);
                let cursor_pos = self.component.cursor_pos();
                let _ = self.file_completion.handle_input(c, cursor_pos);
                // Return Redraw to ensure the file completion list updates immediately
                Msg::Redraw
            }
            // Backspace: let FileCompletion handle it
            Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                let cursor_pos = self.component.cursor_pos();
                // Cancel completion if cursor moved before @ symbol or handle_input returns false
                if cursor_pos < self.file_completion.query_start_pos()
                    || !self.file_completion.handle_input('\x08', cursor_pos)
                {
                    self.cancel_file_completion();
                }
                // Return Redraw to ensure the file completion list updates immediately
                Msg::Redraw
            }
            // Readline shortcuts: handle directly to avoid recursion with handle_normal_input
            Event::Keyboard(KeyEvent {
                code: Key::Char('w'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.delete_word_backward();
                self.update_file_completion_after_edit();
                Msg::InputChanged(self.component.content().to_string())
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.kill_to_start_of_line();
                self.update_file_completion_after_edit();
                Msg::InputChanged(self.component.content().to_string())
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('a'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component
                    .move_and_clear_selection(|c| c.move_to_start_of_line());
                // Cancel completion if cursor moved before @
                if self.component.cursor_pos() < self.file_completion.query_start_pos() {
                    self.cancel_file_completion();
                }
                Msg::Redraw
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('e'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component
                    .move_and_clear_selection(|c| c.move_to_end_of_line());
                Msg::Redraw
            }
            _ => Msg::Redraw,
        }
    }

    /// Update file completion state after edit
    fn update_file_completion_after_edit(&mut self) {
        let cursor_pos = self.component.cursor_pos();
        if cursor_pos < self.file_completion.query_start_pos() {
            self.cancel_file_completion();
        } else {
            let content = self.component.content();
            self.file_completion.sync_query(content, cursor_pos);
            self.file_completion.refresh_list();
        }
    }
}
