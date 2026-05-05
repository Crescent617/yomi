//! UI rendering

use tuirealm::props::{AttrValue, Attribute};
use tuirealm::ratatui::layout::{Constraint, Direction, Layout};
use tuirealm::state::{State, StateValue};
use tuirealm::terminal::TerminalAdapter;
use unicode_width::UnicodeWidthStr;

use crate::app::types::Model;
use crate::app::state::AppMode;
use crate::app::ui_ops::UiOps;
use crate::id::Id;

/// Rendering trait
pub trait Render {
    fn view(&mut self);
}

impl Render for Model {
    fn view(&mut self) {
        // Update scroll progress on each redraw
        self.update_scroll_progress();

        // Pre-fetch content to calculate height without borrowing self in closure
        let input_content =
            if let Ok(State::Single(StateValue::String(content))) = self.app.state(&Id::InputBox) {
                content
            } else {
                String::new()
            };

        let mode = self.get_current_mode();

        let _ = self.terminal.draw(|f| {
            let input_height =
                Self::calculate_input_height_for_content(&input_content, f.area().width);

            if mode == AppMode::Browse {
                // Browse mode: full screen chat view with status bar
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(
                        [
                            Constraint::Min(3),
                            Constraint::Length(1),
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                self.app.view(&Id::ChatView, f, chunks[0]);
                self.app.view(&Id::StatusBar, f, chunks[1]);
            } else {
                // Normal mode: show all components
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(
                        [
                            Constraint::Min(3),
                            Constraint::Length(1),
                            Constraint::Length(input_height),
                            Constraint::Length(1),
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                self.app.view(&Id::ChatView, f, chunks[0]);
                self.app.view(&Id::InfoBar, f, chunks[1]);
                self.app.view(&Id::InputBox, f, chunks[2]);
                self.app.view(&Id::StatusBar, f, chunks[3]);
            }

            // Render overlays
            self.app.view(&Id::Dialog, f, f.area());
            self.app.view(&Id::HistoryPicker, f, f.area());
            self.app.view(&Id::SessionPicker, f, f.area());
            self.app.view(&Id::HelpDialog, f, f.area());
            self.app.view(&Id::TodoList, f, f.area());
        });
    }
}

/// Layout helper methods
pub trait LayoutOps {
    fn calculate_input_height_for_content(content: &str, terminal_width: u16) -> u16;
    fn get_current_mode(&self) -> AppMode;
}

impl LayoutOps for Model {
    fn calculate_input_height_for_content(content: &str, terminal_width: u16) -> u16 {
        // Account for borders and padding in the layout
        let content_width = (terminal_width.saturating_sub(2) as usize).max(1);

        let visual_lines = if content.is_empty() {
            1
        } else {
            let lines: Vec<&str> = content.split('\n').collect();
            let mut total_visual_lines = 0;

            for line in lines {
                let line_width = line.width();
                let wrapped_lines = line_width.saturating_add(content_width).saturating_sub(1)
                    / content_width.max(1);
                total_visual_lines += wrapped_lines.max(1);
            }

            total_visual_lines.clamp(1, 8)
        };

        visual_lines as u16 + 2 // Add 2 for top/bottom borders
    }

    fn get_current_mode(&self) -> AppMode {
        // Query mode from StatusBar component
        if let Ok(Some(query_result)) =
            self.app.query(&Id::StatusBar, Attribute::Custom(crate::attr::MODE))
        {
            if let AttrValue::Number(mode_val) = query_result.into_attr() {
                return if mode_val == 1 {
                    AppMode::Browse
                } else {
                    AppMode::Normal
                };
            }
        }
        AppMode::Normal
    }
}
