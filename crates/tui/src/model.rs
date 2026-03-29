//! Application state model

use ratatui::style::Style;

pub type MessageId = usize;

/// Chat message roles
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

/// Tool call information
#[derive(Debug, Clone, Default)]
pub struct ToolCall {
    pub name: String,
    pub input: String,
    pub output: String,
}

/// A chat message with all metadata
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: MessageId,
    pub role: Role,
    pub content: String,
    pub thinking: Option<String>,
    pub thinking_folded: bool,
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    pub fn new(id: MessageId, role: Role, content: String) -> Self {
        Self {
            id,
            role,
            content,
            thinking: None,
            thinking_folded: true,
            tool_calls: Vec::new(),
        }
    }

    pub fn with_thinking(mut self, thinking: String) -> Self {
        self.thinking = Some(thinking);
        self
    }

    pub fn style(&self) -> Style {
        use crate::theme::Styles;
        match self.role {
            Role::User => Styles::user_content(),
            Role::Assistant => Styles::assistant_content(),
            Role::System => Styles::system(),
        }
    }

    pub fn prefix(&self) -> &'static str {
        match self.role {
            Role::User => "❯",
            Role::Assistant => "◆",
            Role::System => "▪",
        }
    }
}

/// Input buffer state with cursor and history
#[derive(Debug, Clone)]
pub struct InputState {
    pub content: String,
    pub cursor_pos: usize,
    pub history: Vec<String>,
    pub history_index: Option<usize>,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            content: String::new(),
            cursor_pos: 0,
            history: Vec::new(),
            history_index: None,
        }
    }
}

impl InputState {
    pub fn insert(&mut self, c: char) {
        self.content.insert(self.cursor_pos, c);
        self.cursor_pos += 1;
    }

    pub fn insert_newline(&mut self) {
        self.insert('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.content.remove(self.cursor_pos);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor_pos < self.content.len() {
            self.content.remove(self.cursor_pos);
        }
    }

    pub fn delete_word(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let end = self.cursor_pos;
        // Skip whitespace
        while self.cursor_pos > 0
            && self
                .content
                .chars()
                .nth(self.cursor_pos - 1)
                .map_or(false, |c| c.is_whitespace())
        {
            self.cursor_pos -= 1;
        }
        // Delete word chars
        while self.cursor_pos > 0
            && self
                .content
                .chars()
                .nth(self.cursor_pos - 1)
                .map_or(false, |c| !c.is_whitespace())
        {
            self.cursor_pos -= 1;
        }
        self.content.drain(self.cursor_pos..end);
    }

    pub fn delete_to_start(&mut self) {
        self.content.drain(0..self.cursor_pos);
        self.cursor_pos = 0;
    }

    pub fn delete_to_end(&mut self) {
        self.content.truncate(self.cursor_pos);
    }

    pub fn move_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_pos < self.content.len() {
            self.cursor_pos += 1;
        }
    }

    pub fn move_to_start(&mut self) {
        self.cursor_pos = 0;
    }

    pub fn move_to_end(&mut self) {
        self.cursor_pos = self.content.len();
    }

    pub fn move_up(&mut self) {
        // Find previous newline
        if let Some(pos) = self.content[..self.cursor_pos].rfind('\n') {
            self.cursor_pos = pos;
        }
    }

    pub fn move_down(&mut self) {
        // Find next newline
        if let Some(pos) = self.content[self.cursor_pos..].find('\n') {
            self.cursor_pos = self.cursor_pos + pos + 1;
        }
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.history_index = Some(self.history.len() - 1);
                self.content = self.history.last().unwrap().clone();
                self.cursor_pos = self.content.len();
            }
            Some(0) => {} // Already at oldest
            Some(idx) => {
                self.history_index = Some(idx - 1);
                self.content = self.history[idx - 1].clone();
                self.cursor_pos = self.content.len();
            }
        }
    }

    pub fn history_next(&mut self) {
        match self.history_index {
            None => {} // Not in history mode
            Some(idx) if idx >= self.history.len() - 1 => {
                // Return to empty
                self.history_index = None;
                self.content.clear();
                self.cursor_pos = 0;
            }
            Some(idx) => {
                self.history_index = Some(idx + 1);
                self.content = self.history[idx + 1].clone();
                self.cursor_pos = self.content.len();
            }
        }
    }

    pub fn commit(&mut self) -> String {
        let content = self.content.clone();
        if !content.is_empty() {
            self.history.push(content.clone());
        }
        self.content.clear();
        self.cursor_pos = 0;
        self.history_index = None;
        content
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub fn lines(&self) -> Vec<&str> {
        self.content.lines().collect()
    }
}

/// Streaming state for AI response
#[derive(Debug, Clone, Default)]
pub struct StreamingState {
    pub is_active: bool,
    pub content: String,
    pub thinking: String,
    pub spinner_frame: usize,
}

/// Application state model
#[derive(Debug, Clone)]
pub struct Model {
    pub messages: Vec<ChatMessage>,
    pub input: InputState,
    pub streaming: StreamingState,
    pub scroll_offset: usize,
    pub next_msg_id: MessageId,
    pub last_ctrl_c: Option<std::time::Instant>,
    pub should_quit: bool,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            input: InputState::default(),
            streaming: StreamingState::default(),
            scroll_offset: 0,
            next_msg_id: 0,
            last_ctrl_c: None,
            should_quit: false,
        }
    }
}

impl Model {
    pub fn add_user_message(&mut self, content: String) -> MessageId {
        let id = self.next_msg_id;
        self.next_msg_id += 1;
        self.messages.push(ChatMessage::new(id, Role::User, content));
        id
    }

    pub fn add_assistant_message(&mut self, content: String, thinking: Option<String>) -> MessageId {
        let id = self.next_msg_id;
        self.next_msg_id += 1;
        let msg = ChatMessage::new(id, Role::Assistant, content).with_thinking(thinking.unwrap_or_default());
        self.messages.push(msg);
        id
    }

    pub fn add_system_message(&mut self, content: String) -> MessageId {
        let id = self.next_msg_id;
        self.next_msg_id += 1;
        self.messages.push(ChatMessage::new(id, Role::System, content));
        id
    }

    pub fn start_streaming(&mut self) {
        self.streaming = StreamingState {
            is_active: true,
            ..Default::default()
        };
    }

    pub fn append_stream_content(&mut self, text: &str) {
        self.streaming.content.push_str(text);
    }

    pub fn append_stream_thinking(&mut self, thinking: &str) {
        self.streaming.thinking.push_str(thinking);
    }

    pub fn stop_streaming(&mut self) -> (String, String) {
        let content = self.streaming.content.clone();
        let thinking = self.streaming.thinking.clone();
        self.streaming = StreamingState::default();
        (content, thinking)
    }

    pub fn toggle_fold(&mut self, msg_id: MessageId) {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == msg_id) {
            msg.thinking_folded = !msg.thinking_folded;
        }
    }

    pub fn scroll(&mut self, delta: i32) {
        if delta < 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub((-delta) as usize);
        } else {
            self.scroll_offset = (self.scroll_offset + delta as usize)
                .min(self.messages.len().saturating_sub(1));
        }
    }
}
