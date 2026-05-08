//! InputComponent definition and Component/AppComponent implementations

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, QueryResult},
    ratatui::{layout::Rect, Frame},
    state::State,
};

use crate::{
    attr,
    components::{input_edit::TextInput, CompletionList, FileCompletion},
    msg::Msg,
    theme::colors,
};

use super::editor::InputEditor;

/// Input component that handles keyboard events
/// Mode is passed from Model via attr
pub struct InputComponent {
    pub(crate) component: InputEditor,
    pub(crate) mode: crate::app::AppMode,
    // History fields
    pub(crate) history: Vec<String>,
    pub(crate) history_index: Option<usize>, // None = new input, Some(i) = editing history[i]
    pub(crate) saved_input: String,          // Buffer for current input when browsing history
    // Command completion
    pub(crate) command_completion: CompletionList<(String, String)>,
    pub(crate) command_query: String,    // Current query string (text after /)
    pub(crate) command_start_pos: usize, // Position of '/' in the input
    // File completion (@-mention)
    pub(crate) file_completion: FileCompletion,
    // Paste support (images and text)
    pub(crate) placeholder_counter: usize,
    pub(crate) image_paths: std::collections::HashMap<String, std::path::PathBuf>,
    pub(crate) pasted_contents: std::collections::HashMap<String, String>,
}

impl Default for InputComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl InputComponent {
    pub fn new() -> Self {
        Self {
            component: InputEditor::new(),
            mode: crate::app::AppMode::Normal,
            history: Vec::new(),
            history_index: None,
            saved_input: String::new(),
            command_completion: CompletionList::new(),
            command_query: String::new(),
            command_start_pos: 0,
            file_completion: FileCompletion::new(),
            placeholder_counter: 0,
            image_paths: std::collections::HashMap::new(),
            pasted_contents: std::collections::HashMap::new(),
        }
    }

    /// Set the working directory for file completion
    pub fn set_working_dir(&mut self, path: impl Into<std::path::PathBuf>) {
        self.file_completion.set_working_dir(path);
    }

    /// Set the current mode
    pub const fn set_mode(&mut self, mode: crate::app::AppMode) {
        self.mode = mode;
    }

    /// Calculate the number of visual lines needed for the current content
    /// given a specific content width (accounting for wrapping)
    pub fn calculate_visual_lines(&self, content_width: usize) -> usize {
        let visual_lines = self.component.wrap_lines(content_width.max(1));
        visual_lines.len()
    }
}

impl Component for InputComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Real-time update for file completion (refresh from async scan)
        if self.file_completion.is_active() {
            self.file_completion.refresh_list();
        }

        // Render command completion using generic helper
        Self::render_completion_dropdown(
            &mut self.command_completion,
            frame,
            area,
            6, // MAX_VISIBLE_ITEMS
            0, // No footer
            |(cmd, desc), i, selected_idx| {
                let is_selected = i == selected_idx;
                let cmd_style = if is_selected {
                    tuirealm::ratatui::style::Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(tuirealm::ratatui::style::Modifier::BOLD)
                } else {
                    tuirealm::ratatui::style::Style::default().fg(colors::text_primary())
                };
                let desc_style = if is_selected {
                    tuirealm::ratatui::style::Style::default()
                        .fg(colors::text_muted())
                        .add_modifier(tuirealm::ratatui::style::Modifier::BOLD)
                } else {
                    tuirealm::ratatui::style::Style::default().fg(colors::text_muted())
                };
                tuirealm::ratatui::text::Line::from(vec![
                    tuirealm::ratatui::text::Span::styled(cmd.as_str(), cmd_style),
                    tuirealm::ratatui::text::Span::styled("  ", desc_style),
                    tuirealm::ratatui::text::Span::styled(desc.as_str(), desc_style),
                ])
            },
        );

        // Render file completion dropdown (reserves footer space for status line)
        Self::render_completion_dropdown(
            self.file_completion.completion_list_mut(),
            frame,
            area,
            8, // MAX_VISIBLE_FILES
            1, // Reserve space for status line above input
            |file, i, selected_idx| {
                let is_selected = i == selected_idx;
                let style = if is_selected {
                    tuirealm::ratatui::style::Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(tuirealm::ratatui::style::Modifier::BOLD)
                } else {
                    tuirealm::ratatui::style::Style::default().fg(colors::text_primary())
                };
                tuirealm::ratatui::text::Line::from(tuirealm::ratatui::text::Span::styled(
                    file.as_str(),
                    style,
                ))
            },
        );

        // Render file completion status line (after dropdown, at the reserved footer position)
        if self.file_completion.is_visible() && !self.file_completion.is_empty() {
            let total_files = self.file_completion.total_files();
            let truncated_suffix = if self.file_completion.is_truncated() {
                "+"
            } else {
                ""
            };
            let status_text = format!(
                " {} / {}{} files",
                self.file_completion.len(),
                total_files,
                truncated_suffix
            );
            let status_height = 1u16;
            let status_area = Rect {
                x: area.x,
                y: area.y.saturating_sub(status_height),
                width: area.width,
                height: status_height,
            };

            let status_style = tuirealm::ratatui::style::Style::default()
                .fg(colors::text_muted())
                .add_modifier(tuirealm::ratatui::style::Modifier::DIM);
            let status_line = tuirealm::ratatui::text::Line::from(
                tuirealm::ratatui::text::Span::styled(status_text, status_style),
            );
            let status_widget = tuirealm::ratatui::widgets::Paragraph::new(
                tuirealm::ratatui::text::Text::from(vec![status_line]),
            );
            frame.render_widget(status_widget, status_area);
        }

        self.component.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.component.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(attr::MODE) => {
                if let AttrValue::Number(mode_val) = value {
                    self.mode = match mode_val {
                        1 => crate::app::AppMode::Browse,
                        _ => crate::app::AppMode::Normal,
                    };
                }
            }
            Attribute::Custom(attr::HISTORY) => {
                if let AttrValue::String(data) = value {
                    if let Ok(history) = serde_json::from_str::<Vec<String>>(&data) {
                        self.set_history(history);
                    }
                }
            }
            Attribute::Custom(attr::WORKING_DIR) => {
                if let AttrValue::String(path) = value {
                    self.set_working_dir(path);
                }
            }
            Attribute::Custom(attr::INPUT_CONTENT) => {
                if let AttrValue::String(content) = value {
                    self.component.clear();
                    self.component.insert_str(&content);
                }
            }
            _ => self.component.attr(attr, value),
        }
    }

    fn state(&self) -> State {
        self.component.state()
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.component.perform(cmd)
    }
}

impl AppComponent<Msg, crate::msg::UserEvent> for InputComponent {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        self.handle_input(ev)
    }
}
