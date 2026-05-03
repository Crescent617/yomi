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
        widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph},
        Frame,
    },
    state::{State, StateValue},
};

use crate::{
    attr,
    components::input_edit::{TextBuffer, TextInput},
    theme::colors,
};
use unicode_width::UnicodeWidthStr;

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

    #[must_use]
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
    pub min_width: u16,     // Minimum width in columns, default 40
    pub min_height: u16,    // Minimum height in rows, default 10
}

impl Default for PickerConfig {
    fn default() -> Self {
        Self {
            title: "Select".to_string(),
            placeholder: "Search...".to_string(),
            max_list_height: 10,
            width_percent: 0.6,
            min_width: 60,
            min_height: 20,
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

    #[must_use]
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    #[must_use]
    pub fn with_max_height(mut self, height: u16) -> Self {
        self.max_list_height = height;
        self
    }

    #[must_use]
    pub fn with_width_percent(mut self, percent: f32) -> Self {
        self.width_percent = percent.clamp(0.1, 1.0);
        self
    }

    #[must_use]
    pub fn with_min_width(mut self, width: u16) -> Self {
        self.min_width = width;
        self
    }

    #[must_use]
    pub fn with_min_height(mut self, height: u16) -> Self {
        self.min_height = height;
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

    #[must_use]
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
            self.filtered
                .get(self.selected)
                .map(|idx| self.items[*idx].id.clone())
        } else {
            None
        }
    }

    /// Get the currently selected item
    pub fn current_item(&self) -> Option<&PickerItem> {
        if self.visible && !self.filtered.is_empty() {
            self.filtered
                .get(self.selected)
                .map(|idx| &self.items[*idx])
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
    }

    fn select_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        } else {
            self.selected = 0;
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
                        || item
                            .meta
                            .as_ref()
                            .is_some_and(|m| m.to_lowercase().contains(&search_lower))
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

    fn render_picker(&mut self, frame: &mut Frame, area: Rect) {
        // Calculate width: percentage-based with min/max constraints
        let percent_width = (f32::from(area.width) * self.config.width_percent) as u16;
        let palette_width = percent_width
            .max(self.config.min_width)
            .min(area.width.saturating_sub(4));

        // Calculate height: leave 2 rows margin top + 4 rows margin bottom
        let palette_height = area.height.saturating_sub(6);

        let palette_area = Rect {
            x: area.x + (area.width - palette_width) / 2,
            y: area.y + 2, // 2 rows margin from top
            width: palette_width,
            height: palette_height,
        };

        frame.render_widget(Clear, palette_area);

        let block = Block::default()
            .title(self.config.title.as_str())
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
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

        // Filtered items list: fill available height
        // -4: 2 (top/bottom borders) + 2 (search input + separator)
        let max_visible = (palette_area.height.saturating_sub(4)) as usize;

        // Auto-calculate scroll_offset to keep selected item visible
        if self.selected >= self.scroll_offset + max_visible {
            self.scroll_offset = self.selected.saturating_sub(max_visible - 1);
        } else if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
        // Clamp scroll_offset to valid range
        let max_scroll = self.filtered.len().saturating_sub(max_visible);
        self.scroll_offset = self.scroll_offset.min(max_scroll);

        if !self.filtered.is_empty() {
            let scroll = self.scroll_offset;
            let start = scroll;
            let end = (start + max_visible).min(self.filtered.len());

            let mut items: Vec<ListItem> = self
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
                        use std::fmt::Write;
                        write!(content, "  {meta}").ok();
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

            // Fill remaining space with empty items
            for _ in items.len()..max_visible {
                items.push(ListItem::new("").style(Style::default()));
            }

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

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.props.get(attr).map(|v| v.into())
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(attr::DIALOG_SHOW) => {
                self.visible = true;
                self.input.clear();
                self.selected = 0;
                self.scroll_offset = 0;
                self.update_filtered();
            }
            Attribute::Custom(attr::DIALOG_HIDE) => {
                self.hide();
                self.scroll_offset = 0;
            }
            Attribute::Custom(attr::PICKER_ITEMS) => {
                if let AttrValue::Payload(payload) = value {
                    if let Some(any_ref) = payload.as_any() {
                        if let Some(items) = any_ref.downcast_ref::<Vec<PickerItem>>() {
                            self.items.clone_from(items);
                            self.update_filtered();
                        }
                    }
                }
            }
            Attribute::Custom(attr::PICKER_QUERY) => {
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
                CmdResult::Changed(State::Single(StateValue::String(
                    self.input.content().to_string(),
                )))
            }
            Cmd::Delete => {
                self.backspace();
                CmdResult::Changed(State::Single(StateValue::String(
                    self.input.content().to_string(),
                )))
            }
            _ => CmdResult::NoChange,
        }
    }
}

/// Callback type for picker results
pub type PickerCallback = Box<dyn Fn(String) -> crate::msg::Msg + Send>;
pub type PickerCancelCallback = Box<dyn Fn() -> crate::msg::Msg + Send>;

/// App-level fuzzy picker component that handles keyboard events
pub struct FuzzyPickerComponent {
    component: FuzzyPicker,
    on_select: Option<PickerCallback>,
    on_cancel: Option<PickerCancelCallback>,
}

impl std::fmt::Debug for FuzzyPickerComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FuzzyPickerComponent")
            .field("component", &self.component)
            .field("has_on_select", &self.on_select.is_some())
            .field("has_on_cancel", &self.on_cancel.is_some())
            .finish()
    }
}

impl FuzzyPickerComponent {
    pub fn new(config: PickerConfig) -> Self {
        Self {
            component: FuzzyPicker::new(config),
            on_select: None,
            on_cancel: None,
        }
    }

    #[must_use]
    pub fn with_items(mut self, items: Vec<PickerItem>) -> Self {
        self.component = self.component.with_items(items);
        self
    }

    /// Set callbacks for selection and cancel events
    #[must_use]
    pub fn with_callbacks(
        mut self,
        on_select: impl Fn(String) -> crate::msg::Msg + Send + 'static,
        on_cancel: impl Fn() -> crate::msg::Msg + Send + 'static,
    ) -> Self {
        self.on_select = Some(Box::new(on_select));
        self.on_cancel = Some(Box::new(on_cancel));
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

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
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
                    // Use callback if set
                    self.on_select.as_ref().map(|callback| callback(id))
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
                // Use callback if set
                self.on_cancel.as_ref().map(|callback| callback())
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
/// Used with `AppComponent` to send messages back to the app
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerResult {
    Selected(String), // Selected item ID
    Cancelled,
}
