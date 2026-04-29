//! Todo list floating panel component
//!
//! Displays pending and in-progress todos from todoWrite tool on the right side.

use serde::Deserialize;
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

/// Todo item status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// A todo item
#[derive(Debug, Clone, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

/// Todo list data structure for JSON parsing
#[derive(Debug, Clone, Deserialize)]
struct TodoListData {
    todos: Vec<TodoItem>,
}

/// Todo list floating panel component
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
        // Update visible based on state
        self.update_visible();
    }

    /// Update visible state based on todos and manual hide
    fn update_visible(&mut self) {
        self.visible = !self.todos.is_empty() && !self.manually_hidden;
    }

    /// Update todo list from JSON string
    pub fn update_todos(&mut self, json_str: &str) {
        match serde_json::from_str::<TodoListData>(json_str) {
            Ok(data) => {
                // Show all todos (completed items shown with strikethrough)
                self.todos = data.todos;
                self.update_visible();
            }
            Err(e) => {
                tracing::debug!("Failed to parse todo list: {}", e);
                self.visible = false;
            }
        }
    }

    /// Clear todo list and hide
    pub fn clear(&mut self) {
        self.todos.clear();
        self.visible = false;
        self.manually_hidden = false; // Reset manual hide on clear
    }

    /// Check if panel should be visible
    pub fn is_visible(&self) -> bool {
        self.visible && !self.todos.is_empty()
    }

    /// Get the number of pending/in-progress todos
    pub fn pending_count(&self) -> usize {
        self.todos
            .iter()
            .filter(|t| matches!(t.status, TodoStatus::Pending | TodoStatus::InProgress))
            .count()
    }
}

/// Maximum number of todos to display
const MAX_DISPLAY_TODOS: usize = 12;
/// Maximum panel width
const MAX_PANEL_WIDTH: u16 = 40;
/// Minimum screen width to show the panel
const MIN_SCREEN_WIDTH: u16 = 80;
/// Margin for borders: border(2) + `right_spacing(1)` = 3
const PANEL_MARGIN: u16 = 3;
/// Icon width: "○ " or "● " = 2 chars
const ICON_WIDTH: usize = 2;

impl Component for TodoList {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if !self.is_visible() {
            return;
        }

        // Minimum width requirement - if screen is too narrow, don't show
        if area.width < MIN_SCREEN_WIDTH {
            return;
        }

        // Sort todos: incomplete first, completed last
        let mut sorted_todos: Vec<_> = self.todos.clone();
        sorted_todos.sort_by_key(|t| matches!(t.status, TodoStatus::Completed));

        let total_todos = sorted_todos.len();
        let display_count = total_todos.min(MAX_DISPLAY_TODOS);
        let hidden_count = total_todos.saturating_sub(MAX_DISPLAY_TODOS);

        // Calculate content width based on longest todo entry
        let max_content_width = sorted_todos
            .iter()
            .take(MAX_DISPLAY_TODOS)
            .map(|todo| ICON_WIDTH + unicode_width::UnicodeWidthStr::width(todo.content.as_str()))
            .max()
            .unwrap_or(10);

        // Panel width: content + margin, but not exceeding max or screen limit
        let content_with_margin = (max_content_width as u16) + PANEL_MARGIN;
        let panel_width = content_with_margin.min(MAX_PANEL_WIDTH).min(area.width / 3);

        // Calculate height: items + border(2) + optional more indicator(1)
        let panel_height =
            (display_count as u16 + 2 + u16::from(hidden_count > 0)).min(area.height / 2);

        // Position on the right side, top corner
        let panel_area = Rect {
            x: area.x + area.width.saturating_sub(panel_width + 2),
            y: area.y + 1,
            width: panel_width,
            height: panel_height,
        };

        // Clear background
        Clear.render(panel_area, frame.buffer_mut());

        // Build list items
        // Account for borders(2) + right_spacing(1)
        let max_chars = (panel_width as usize).saturating_sub(PANEL_MARGIN as usize);
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
                let truncated = truncate_by_chars(&content, max_chars);

                ListItem::new(Line::from(vec![Span::styled(truncated, style)]))
            })
            .collect();

        // Add "+X more..." indicator if there are hidden todos
        if hidden_count > 0 {
            let more_style = Style::default()
                .fg(colors::text_muted())
                .add_modifier(Modifier::ITALIC);
            items.push(ListItem::new(Line::from(vec![Span::styled(
                format!("+{hidden_count} more..."),
                more_style,
            )])));
        }

        let list = List::new(items)
            .block(
                Block::default()
                    .title("Todos")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(colors::accent_system())),
            )
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));

        frame.render_widget(list, panel_area);
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
mod tests {
    use super::*;

    #[test]
    fn test_parse_todo_json() {
        let json = r#"{"todos":[{"id":"1","content":"Fix bug","status":"pending"},{"id":"2","content":"Write tests","status":"in_progress"}]}"#;
        let data: TodoListData = serde_json::from_str(json).unwrap();
        assert_eq!(data.todos.len(), 2);
        assert_eq!(data.todos[0].id, "1");
        assert_eq!(data.todos[0].content, "Fix bug");
        assert_eq!(data.todos[0].status, TodoStatus::Pending);
        assert_eq!(data.todos[1].id, "2");
        assert_eq!(data.todos[1].status, TodoStatus::InProgress);
    }

    #[test]
    fn test_parse_todo_with_completed() {
        let json = r#"{"todos":[{"id":"1","content":"Done task","status":"completed"},{"id":"2","content":"Pending task","status":"pending"}]}"#;
        let data: TodoListData = serde_json::from_str(json).unwrap();
        assert_eq!(data.todos.len(), 2);
        assert_eq!(data.todos[0].status, TodoStatus::Completed);
        assert_eq!(data.todos[1].status, TodoStatus::Pending);
    }

    #[test]
    fn test_todo_list_shows_completed_with_strikethrough() {
        let json = r#"{"todos":[{"id":"1","content":"Done","status":"completed"},{"id":"2","content":"Pending","status":"pending"}]}"#;
        let mut list = TodoList::new();
        list.update_todos(json);
        // Both completed and pending should be shown (order depends on view sorting)
        assert_eq!(list.todos.len(), 2);
    }

    #[test]
    fn test_parse_todo_with_unicode() {
        let json =
            r#"{"todos":[{"id":"1","content":"演示todo工具的基本功能","status":"in_progress"}]}"#;
        let mut list = TodoList::new();
        list.update_todos(json);
        assert_eq!(list.todos.len(), 1);
        assert_eq!(list.todos[0].content, "演示todo工具的基本功能");
        assert_eq!(list.todos[0].status, TodoStatus::InProgress);
    }

    #[test]
    fn test_parse_todo_with_escapes() {
        let json =
            r#"{"todos":[{"id":"1","content":"Line 1\nLine 2\tTabbed","status":"pending"}]}"#;
        let mut list = TodoList::new();
        list.update_todos(json);
        assert_eq!(list.todos.len(), 1);
        assert_eq!(list.todos[0].content, "Line 1\nLine 2\tTabbed");
    }
}
