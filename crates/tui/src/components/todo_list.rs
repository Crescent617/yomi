//! Todo list floating panel component
//!
//! Displays active goal and pending todos on the right side.
//! Title is always "Tasks".
//! If a goal exists, it renders at the top (wrapped, max 3 lines).
//! Todos render below with a separator line.

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
        widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Widget},
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
/// Maximum goal lines to show
const MAX_GOAL_LINES: usize = 3;

/// Goal info shown in the task panel
#[derive(Debug, Clone)]
struct GoalInfo {
    description: String,
    status: String,
}

/// Task panel floating component (goal + todos)
#[derive(Debug, Default)]
pub struct TodoList {
    todos: Vec<TodoItem>,
    visible: bool,
    /// User manually toggled visibility (overrides auto-show)
    manually_hidden: bool,
    /// Current goal (rendered at the top of the panel)
    goal: Option<GoalInfo>,
}

impl TodoList {
    pub fn new() -> Self {
        Self {
            todos: Vec::new(),
            visible: false,
            manually_hidden: false,
            goal: None,
        }
    }

    /// Toggle visibility (user command)
    pub fn toggle(&mut self) {
        self.manually_hidden = !self.manually_hidden;
        self.update_visible();
    }

    /// Update visible state based on content and manual hide
    /// Show when there are pending todos OR an active goal
    fn update_visible(&mut self) {
        let has_content = self
            .todos
            .iter()
            .any(|t| matches!(t.status, TodoStatus::Pending | TodoStatus::InProgress))
            || self.goal.is_some();
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
        self.goal = None;
    }

    /// Update goal info from raw string "status\0description"
    pub fn update_goal(&mut self, value: &str) {
        let parts: Vec<&str> = value.split('\x00').collect();
        if parts.len() == 2 && !parts[0].is_empty() {
            self.goal = Some(GoalInfo {
                status: parts[0].to_string(),
                description: parts[1].to_string(),
            });
        } else {
            self.goal = None;
        }
        self.update_visible();
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

        let goal_width = if let Some(ref goal) = self.goal {
            let text = format!("🎯 {}", goal.description);
            unicode_width::UnicodeWidthStr::width(text.as_str())
        } else {
            0
        };

        let max_content_width = max_todo_width.max(goal_width);
        let panel_width = (max_content_width as u16 + PANEL_MARGIN).min(area.width * 2 / 5);

        // Estimate goal height (max 3 lines)
        let estimated_inner_width = panel_width.saturating_sub(2) as usize;
        let estimated_goal_lines = if let Some(ref goal) = self.goal {
            let text = format!("🎯 {}", goal.description);
            let text_width = unicode_width::UnicodeWidthStr::width(text.as_str());
            let width = estimated_inner_width.max(1);
            text_width.div_ceil(width).min(MAX_GOAL_LINES) as u16
        } else {
            0
        };

        // Calculate total height
        let mut total_height = 2; // border
        if estimated_goal_lines > 0 {
            total_height += estimated_goal_lines + 1; // +1 for separator
        }
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

        // Render goal at the top
        let mut y = inner.y;
        if let Some(ref goal) = self.goal {
            let status_fg = match goal.status.as_str() {
                "active" => colors::accent_success(),
                "paused" => colors::accent_warning(),
                "blocked" => colors::accent_error(),
                _ => colors::text_muted(),
            };

            let text = format!("🎯 {}", goal.description);
            let text_width = unicode_width::UnicodeWidthStr::width(text.as_str());
            let width = inner.width.max(1) as usize;
            let goal_lines = text_width.div_ceil(width).min(MAX_GOAL_LINES) as u16;

            let goal_line = Line::from(vec![Span::styled(
                format!("🎯 {}", goal.description),
                Style::default()
                    .fg(status_fg)
                    .add_modifier(Modifier::BOLD),
            )]);

            let goal_area = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: goal_lines,
            };

            frame.render_widget(
                Paragraph::new(goal_line).wrap(tuirealm::ratatui::widgets::Wrap { trim: true }),
                goal_area,
            );
            y += goal_lines;

            // Separator between goal and todos
            if !self.todos.is_empty() {
                let sep_area = Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                };
                let sep = "─".repeat(inner.width as usize);
                frame.render_widget(
                    Paragraph::new(Line::from(vec![Span::styled(
                        sep,
                        Style::default().fg(colors::border()),
                    )])),
                    sep_area,
                );
                y += 1;
            }
        }

        // Render todo list below
        if !self.todos.is_empty() {
            let remaining_height = inner.height.saturating_sub(y - inner.y);
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
            } else if *name == attr::SET_GOAL {
                if let AttrValue::String(value_str) = value {
                    self.update_goal(&value_str);
                }
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

    #[test]
    fn test_goal_triggers_visibility() {
        let mut list = TodoList::new();
        assert!(!list.is_visible());

        list.update_goal("active\x00Implement auth");
        assert!(list.is_visible());

        list.update_goal("");
        assert!(!list.is_visible());
    }
}
