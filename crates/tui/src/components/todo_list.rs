//! Todo list floating panel component
//!
//! Displays pending todos on the right side.
//! Title is always "Tasks".

use kernel::storage::todo::TodoListData;
pub use kernel::storage::todo::{TodoItem, TodoStatus};
use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, QueryResult},
    ratatui::{
        layout::Rect,
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::{Block, BorderType, Borders, Clear, List, ListItem, Widget},
        Frame,
    },
    state::State,
};

use crate::{attr, msg::Msg, theme::colors, utils::text::truncate_by_chars};

/// Maximum number of todos to display
const MAX_DISPLAY_TODOS: usize = 12;
/// Minimum screen width to show the panel
const MIN_SCREEN_WIDTH: u16 = 60;
/// Margin for borders: border(2) + `right_spacing(1)` = 3
const PANEL_MARGIN: u16 = 3;
/// Icon width: "○ " or "● " = 2 chars
const ICON_WIDTH: usize = 2;

/// Task panel floating component (todos)
#[derive(Debug, Default)]
pub struct TodoList {
    todos: Vec<TodoItem>,
    visible: bool,
    /// User manually toggled visibility (overrides auto-show)
    manually_hidden: bool,
}

impl TodoList {
    pub fn new() -> Self {
        Self {
            todos: Vec::new(),
            visible: false,
            manually_hidden: false,
        }
    }

    /// Toggle visibility (user command)
    pub fn toggle(&mut self) {
        self.manually_hidden = !self.manually_hidden;
        self.update_visible();
    }

    /// Update visible state based on content and manual hide
    /// Show when there are pending todos
    fn update_visible(&mut self) {
        let has_content = self
            .todos
            .iter()
            .any(|t| matches!(t.status, TodoStatus::Pending | TodoStatus::InProgress));
        self.visible = has_content && !self.manually_hidden;
    }

    /// Update todo list from JSON string
    pub fn update_todos(&mut self, json_str: &str) {
        match serde_json::from_str::<TodoListData>(json_str) {
            Ok(data) => {
                self.todos = data.todos;
                self.update_visible();
            }
            Err(e) => {
                tracing::debug!("Failed to parse todo list: {}", e);
                self.update_visible();
            }
        }
    }

    /// Clear everything and hide
    pub fn clear(&mut self) {
        self.todos.clear();
        self.visible = false;
        self.manually_hidden = false;
    }

    /// Check if panel should be visible
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get the number of pending/in-progress todos
    pub fn pending_count(&self) -> usize {
        self.todos
            .iter()
            .filter(|t| matches!(t.status, TodoStatus::Pending | TodoStatus::InProgress))
            .count()
    }
}

impl Component for TodoList {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if !self.is_visible() {
            return;
        }
        if area.width < MIN_SCREEN_WIDTH {
            return;
        }

        // Sort todos: incomplete first, completed last
        let mut sorted_todos: Vec<_> = self.todos.clone();
        sorted_todos.sort_by_key(|t| matches!(t.status, TodoStatus::Completed));

        let total_todos = sorted_todos.len();
        let display_todos = total_todos.min(MAX_DISPLAY_TODOS);
        let hidden_todos = total_todos.saturating_sub(MAX_DISPLAY_TODOS);

        // Calculate panel width based on longest content
        let max_todo_width = sorted_todos
            .iter()
            .take(MAX_DISPLAY_TODOS)
            .map(|todo| ICON_WIDTH + unicode_width::UnicodeWidthStr::width(todo.content.as_str()))
            .max()
            .unwrap_or(0);

        let panel_width = (max_todo_width as u16 + PANEL_MARGIN).min(area.width * 2 / 5);

        // Calculate total height
        let mut total_height = 2; // border
        total_height += display_todos as u16;
        if hidden_todos > 0 {
            total_height += 1;
        }
        total_height = total_height.min(area.height / 2);

        // Position on the right side, top corner
        let panel_area = Rect {
            x: area.x + area.width.saturating_sub(panel_width),
            y: area.y,
            width: panel_width,
            height: total_height,
        };

        // Clear background
        Clear.render(panel_area, frame.buffer_mut());

        // Render border block
        let block = Block::default()
            .title("Tasks")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(colors::accent_system()));
        frame.render_widget(block.clone(), panel_area);

        let inner = block.inner(panel_area);

        // Render todo list
        if !self.todos.is_empty() {
            let y = inner.y;
            let remaining_height = inner.height;
            let list_area = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: remaining_height,
            };

            let mut items: Vec<ListItem> = sorted_todos
                .iter()
                .take(MAX_DISPLAY_TODOS)
                .map(|todo| {
                    let (icon, style) = match todo.status {
                        TodoStatus::Pending => ("○", Style::default().fg(colors::text_primary())),
                        TodoStatus::InProgress => (
                            "●",
                            Style::default()
                                .fg(colors::accent_success())
                                .add_modifier(Modifier::BOLD),
                        ),
                        TodoStatus::Completed => (
                            "●",
                            Style::default()
                                .fg(colors::text_muted())
                                .add_modifier(Modifier::CROSSED_OUT),
                        ),
                    };

                    let content = format!("{} {}", icon, todo.content);
                    let truncated = truncate_by_chars(&content, inner.width as usize);

                    ListItem::new(Line::from(vec![Span::styled(truncated, style)]))
                })
                .collect();

            if hidden_todos > 0 {
                let more_style = Style::default()
                    .fg(colors::text_muted())
                    .add_modifier(Modifier::ITALIC);
                items.push(ListItem::new(Line::from(vec![Span::styled(
                    format!("+{hidden_todos} more..."),
                    more_style,
                )])));
            }

            let list =
                List::new(items).highlight_style(Style::default().add_modifier(Modifier::BOLD));
            frame.render_widget(list, list_area);
        }
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        if let Attribute::Custom(name) = &attr {
            if *name == attr::SET_TODOS {
                if let AttrValue::String(json_str) = value {
                    self.update_todos(&json_str);
                }
            } else if *name == attr::CLEAR_TODOS {
                self.clear();
            } else if *name == attr::TOGGLE_TODOS {
                self.toggle();
            }
        }
    }

    fn state(&self) -> State {
        State::None
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}

/// Component wrapper for `TodoList`
pub struct TodoListComponent {
    component: TodoList,
}

impl Default for TodoListComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoListComponent {
    pub fn new() -> Self {
        Self {
            component: TodoList::new(),
        }
    }
}

impl Component for TodoListComponent {
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

impl AppComponent<Msg, crate::msg::UserEvent> for TodoListComponent {
    fn on(&mut self, _ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        None
    }
}

#[cfg(test)]
#[path = "todo_list_test.rs"]
mod tests;
