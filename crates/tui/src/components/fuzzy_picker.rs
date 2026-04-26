//! Generic fuzzy picker component for TUI
//!
//! A reusable component for fuzzy searching through a list of items.
//! Similar to telescope/fzf, can be used for:
//! - History search (C-r)
//! - Command palette
//! - File picker
//! - Any list that needs filtering

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
        style::{Modifier, Style},
        widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
        Frame,
    },
    state::{State, StateValue},
};

use unicode_width::UnicodeWidthStr;
use crate::{components::input_edit::{TextBuffer, TextInput}, theme::colors};

/// An item in the fuzzy picker
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
    pub meta: Option<String>, // Optional metadata shown in secondary color
}

impl PickerItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            meta: None,
        }
    }

    pub fn with_meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }
}

/// Configuration for the fuzzy picker appearance
#[derive(Debug, Clone)]
pub struct PickerConfig {
    pub title: String,
    pub placeholder: String,
    pub max_list_height: u16,
    pub width_percent: f32, // 0.0-1.0, defaults to 0.6
}

impl Default for PickerConfig {
    fn default() -> Self {
        Self {
            title: "Select".to_string(),
            placeholder: "Search...".to_string(),
            max_list_height: 10,
            width_percent: 0.6,
        }
    }
}

impl PickerConfig {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn with_max_height(mut self, height: u16) -> Self {
        self.max_list_height = height;
        self
    }

    pub fn with_width_percent(mut self, percent: f32) -> Self {
        self.width_percent = percent.clamp(0.1, 1.0);
        self
    }
}

/// Fuzzy picker component for selecting from a filtered list
#[derive(Debug)]
pub struct FuzzyPicker {
    props: Props,
    config: PickerConfig,
    items: Vec<PickerItem>,
    filtered: Vec<usize>, // Indices into items
    selected: usize,
    scroll_offset: usize, // First visible item index
    input: TextBuffer,
    visible: bool,
}

impl FuzzyPicker {
    pub fn new(config: PickerConfig) -> Self {
        Self {
            props: Props::default(),
            config,
            items: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            input: TextBuffer::new(),
            visible: false,
        }
    }

    pub fn with_items(mut self, items: Vec<PickerItem>) -> Self {
        self.items = items;
        self.filtered = (0..self.items.len()).collect();
        self.selected = 0;
        self
    }

    /// Show the picker with new items
    pub fn show(&mut self, items: Vec<PickerItem>) {
        self.items = items;
        self.input.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.visible = true;
        self.update_filtered();
    }

    /// Hide the picker
    pub fn hide(&mut self) {
        self.visible = false;
        self.input.clear();
        self.scroll_offset = 0;
    }

    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get the currently selected item's id
    pub fn current_selection(&self) -> Option<String> {
        if self.visible && !self.filtered.is_empty() {
            self.filtered.get(self.selected).map(|idx| self.items[*idx].id.clone())
        } else {
            None
        }
    }

    /// Get the currently selected item
    pub fn current_item(&self) -> Option<&PickerItem> {
        if self.visible && !self.filtered.is_empty() {
            self.filtered.get(self.selected).map(|idx| &self.items[*idx])
        } else {
            None
        }
    }

    /// Get the current search query
    pub fn search_query(&self) -> &str {
        self.input.content()
    }

    /// Set the search query
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.input = TextBuffer::with_content(query);
        self.selected = 0;
        self.update_filtered();
    }

    /// Get mutable access to the input buffer
    pub fn input_mut(&mut self) -> &mut TextBuffer {
        &mut self.input
    }

    fn select_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        } else if !self.filtered.is_empty() {
            self.selected = self.filtered.len() - 1;
        }
        // Sticky scroll: ensure selected item is visible
        self.ensure_selected_visible();
    }

    fn select_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        } else {
            self.selected = 0;
        }
        // Sticky scroll: ensure selected item is visible
        self.ensure_selected_visible();
    }

    /// Ensure the selected item is within the visible range (sticky scrolling)
    fn ensure_selected_visible(&mut self) {
        let max_visible = self.config.max_list_height as usize;

        if self.selected < self.scroll_offset {
            // Selection moved above visible area, scroll up
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + max_visible {
            // Selection moved below visible area, scroll down
            self.scroll_offset = self.selected.saturating_sub(max_visible - 1);
        }

        // Clamp scroll offset to valid range
        let max_scroll = self.filtered.len().saturating_sub(max_visible);
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }
    }

    fn update_filtered(&mut self) {
        let search_lower = self.input.content().to_lowercase();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if search_lower.is_empty() {
                    true
                } else {
                    item.label.to_lowercase().contains(&search_lower)
                        || item.id.to_lowercase().contains(&search_lower)
                        || item.meta.as_ref().is_some_and(|m| {
                            m.to_lowercase().contains(&search_lower)
                        })
                }
            })
            .map(|(idx, _)| idx)
            .collect();

        // Reset selection and scroll when filter changes
        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        self.input.insert_char(c);
        self.update_filtered();
    }

    pub fn backspace(&mut self) {
        self.input.backspace();
        self.update_filtered();
    }

    fn render_picker(&self, frame: &mut Frame, area: Rect) {
        let palette_width = (f32::from(area.width) * self.config.width_percent).clamp(40.0, 80.0) as u16;
        let search_height = 3u16;
        let list_height = (self.filtered.len() as u16).min(self.config.max_list_height);
        let palette_height = search_height + list_height + 2;

        let palette_area = Rect {
            x: area.x + (area.width - palette_width) / 2,
            y: area.y + (area.height - palette_height) / 3,
            width: palette_width,
            height: palette_height,
        };

        frame.render_widget(Clear, palette_area);

        let block = Block::default()
            .title(self.config.title.clone())
            .borders(Borders::ALL)
            .border_style(colors::accent_system())
            .title_style(
                Style::default()
                    .fg(colors::accent_system())
                    .add_modifier(Modifier::BOLD),
            );

        let inner = palette_area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        // Search input with placeholder
        let search_text = if self.input.is_empty() {
            format!("> {}", self.config.placeholder)
        } else {
            format!("> {}", self.input.content())
        };
        let search_style = if self.input.is_empty() {
            Style::default().fg(colors::text_muted())
        } else {
            Style::default()
                .fg(colors::text_primary())
                .add_modifier(Modifier::BOLD)
        };
        let search_para = Paragraph::new(search_text).style(search_style);
        frame.render_widget(search_para, chunks[0]);

        // Separator line
        let separator = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(colors::border()));
        let separator_area = Rect {
            x: inner.x,
            y: chunks[0].y + 1,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(separator, separator_area);

        // Filtered items list (with sticky scrolling)
        if !self.filtered.is_empty() {
            let max_visible = self.config.max_list_height as usize;
            // Ensure scroll_offset is valid
            let max_scroll = self.filtered.len().saturating_sub(max_visible);
            let scroll = self.scroll_offset.min(max_scroll);
            let start = scroll;
            let end = (start + max_visible).min(self.filtered.len());

            let items: Vec<ListItem> = self
                .filtered
                .iter()
                .skip(start)
                .take(end - start)
                .enumerate()
                .map(|(display_idx, item_idx)| {
                    let actual_idx = start + display_idx; // Actual index in filtered list
                    let item = &self.items[*item_idx];
                    let is_selected = actual_idx == self.selected;
                    let prefix = if is_selected { "▸ " } else { "  " };

                    let mut content = format!("{}{}", prefix, item.label);
                    if let Some(meta) = &item.meta {
                        content.push_str(&format!("  {}", meta));
                    }

                    let style = if is_selected {
                        Style::default()
                            .fg(colors::accent_system())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(colors::text_primary())
                    };

                    ListItem::new(content).style(style)
                })
                .collect();

            let list = List::new(items).block(Block::default());
            frame.render_widget(list, chunks[1]);
        } else if !self.input.is_empty() {
            let no_results = Paragraph::new("No matches found")
                .alignment(Alignment::Center)
                .style(Style::default().fg(colors::text_muted()));
            frame.render_widget(no_results, chunks[1]);
        }

        frame.render_widget(block, palette_area);

        // Set cursor position (only if not showing placeholder)
        if !self.input.is_empty() {
            let search_width = self.input.content().width() as u16;
            let cursor_x = chunks[0].x + 2 + search_width;
            let cursor_y = chunks[0].y;
            frame.set_cursor_position(tuirealm::ratatui::layout::Position::new(cursor_x, cursor_y));
        }
    }
}

impl Component for FuzzyPicker {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if self.visible {
            self.render_picker(frame, area);
        }
    }

    fn query<'a>(&'a self, attr: Attribute) -> Option<QueryResult<'a>> {
        self.props.get(attr).map(|v| v.into())
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom("show") => {
                self.visible = true;
                self.input.clear();
                self.selected = 0;
                self.scroll_offset = 0;
                self.update_filtered();
            }
            Attribute::Custom("hide") => {
                self.hide();
                self.scroll_offset = 0;
            }
            Attribute::Custom("items") => {
                if let AttrValue::Payload(payload) = value {
                    if let Some(any_ref) = payload.as_any() {
                        if let Some(items) = any_ref.downcast_ref::<Vec<PickerItem>>() {
                            self.items.clone_from(items);
                            self.update_filtered();
                        }
                    }
                }
            }
            Attribute::Custom("query") => {
                if let AttrValue::String(query) = value {
                    self.set_query(query);
                }
            }
            _ => {
                self.props.set(attr, value);
            }
        }
    }

    fn state(&self) -> State {
        if let Some(id) = self.current_selection() {
            State::Single(StateValue::String(id))
        } else {
            State::None
        }
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        if !self.visible {
            return CmdResult::NoChange;
        }

        match cmd {
            Cmd::Move(tuirealm::command::Direction::Up) => {
                self.select_up();
                CmdResult::Changed(State::Single(StateValue::Usize(self.selected)))
            }
            Cmd::Move(tuirealm::command::Direction::Down) => {
                self.select_down();
                CmdResult::Changed(State::Single(StateValue::Usize(self.selected)))
            }
            Cmd::Submit => {
                if let Some(id) = self.current_selection() {
                    self.hide();
                    CmdResult::Submit(State::Single(StateValue::String(id)))
                } else {
                    CmdResult::NoChange
                }
            }
            Cmd::Cancel => {
                self.hide();
                CmdResult::Submit(State::None)
            }
            Cmd::Type(c) => {
                self.insert_char(c);
                CmdResult::Changed(State::Single(StateValue::String(self.input.content().to_string())))
            }
            Cmd::Delete => {
                self.backspace();
                CmdResult::Changed(State::Single(StateValue::String(self.input.content().to_string())))
            }
            _ => CmdResult::NoChange,
        }
    }
}

/// App-level fuzzy picker component that handles keyboard events
#[derive(Debug)]
pub struct FuzzyPickerComponent {
    component: FuzzyPicker,
}

impl FuzzyPickerComponent {
    pub fn new(config: PickerConfig) -> Self {
        Self {
            component: FuzzyPicker::new(config),
        }
    }

    pub fn with_items(mut self, items: Vec<PickerItem>) -> Self {
        self.component = self.component.with_items(items);
        self
    }

    pub fn show(&mut self, items: Vec<PickerItem>) {
        self.component.show(items);
    }

    pub fn hide(&mut self) {
        self.component.hide();
    }

    pub const fn is_visible(&self) -> bool {
        self.component.is_visible()
    }

    pub fn current_selection(&self) -> Option<String> {
        self.component.current_selection()
    }

    pub fn current_item(&self) -> Option<&PickerItem> {
        self.component.current_item()
    }
}

impl Component for FuzzyPickerComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.component.view(frame, area);
    }

    fn query<'a>(&'a self, attr: Attribute) -> Option<QueryResult<'a>> {
        self.component.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.component.attr(attr, value);
    }

    fn state(&self) -> State {
        self.component.state()
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.component.perform(cmd)
    }
}

impl AppComponent<crate::msg::Msg, crate::msg::UserEvent> for FuzzyPickerComponent {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<crate::msg::Msg> {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers};
        use Event::Keyboard;

        if !self.component.is_visible() {
            return None;
        }

        match *ev {
            // Up arrow or Ctrl+P: navigate up
            Keyboard(
                KeyEvent {
                    code: Key::Up,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('p'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component.select_up();
                Some(crate::msg::Msg::Redraw)
            }
            // Down arrow or Ctrl+N: navigate down
            Keyboard(
                KeyEvent {
                    code: Key::Down,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('n'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component.select_down();
                Some(crate::msg::Msg::Redraw)
            }
            // Enter: select current item
            Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => {
                if let Some(id) = self.component.current_selection() {
                    self.component.hide();
                    Some(crate::msg::Msg::HistorySelected(id))
                } else {
                    None
                }
            }
            // Escape or Ctrl+C: close without selection
            Keyboard(
                KeyEvent {
                    code: Key::Esc,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component.hide();
                Some(crate::msg::Msg::CloseHistoryPicker)
            }
            // Regular character: add to search
            Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                self.component.insert_char(c);
                Some(crate::msg::Msg::Redraw)
            }
            // Backspace: delete character
            Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                Some(crate::msg::Msg::Redraw)
            }
            // Ctrl+W: delete word backward
            Keyboard(KeyEvent {
                code: Key::Char('w'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().delete_word_backward();
                self.component.update_filtered();
                Some(crate::msg::Msg::Redraw)
            }
            // Ctrl+U: delete to start of line
            Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().kill_to_start_of_line();
                self.component.update_filtered();
                Some(crate::msg::Msg::Redraw)
            }
            // Ctrl+A: move to start of line
            Keyboard(KeyEvent {
                code: Key::Char('a'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().move_to_start_of_line();
                Some(crate::msg::Msg::Redraw)
            }
            // Ctrl+E: move to end of line
            Keyboard(KeyEvent {
                code: Key::Char('e'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().move_to_end_of_line();
                Some(crate::msg::Msg::Redraw)
            }
            _ => None,
        }
    }
}

/// Generic message type for picker results
/// Used with AppComponent to send messages back to the app
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerResult {
    Selected(String), // Selected item ID
    Cancelled,
}

// Convenience function to create history items
pub fn history_items(history: &[String]) -> Vec<PickerItem> {
    history
        .iter()
        .enumerate()
        .map(|(idx, text)| {
            // Replace newlines with spaces and trim leading whitespace for preview
            let text_single_line = text.replace('\n', " ").trim_start().to_string();
            let preview = if text_single_line.chars().count() > 60 {
                // Safe Unicode-aware truncation: take characters, not bytes
                let truncated: String = text_single_line.chars().take(57).collect();
                format!("{}...", truncated)
            } else {
                text_single_line
            };
            PickerItem::new(format!("history_{}", idx), preview)
        })
        .rev() // Most recent first
        .collect()
}
