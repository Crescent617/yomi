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
    pub const fn new(id: MessageId, role: Role, content: String) -> Self {
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

    pub const fn prefix(&self) -> &'static str {
        match self.role {
            Role::User => "❯",
            Role::Assistant => "◆",
            Role::System => "▪",
        }
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
#[derive(Default)]
pub struct Model {
    pub messages: Vec<ChatMessage>,
    pub streaming: StreamingState,
    pub scroll_offset: usize,
    pub next_msg_id: MessageId,
    pub last_ctrl_c: Option<std::time::Instant>,
    pub should_quit: bool,
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
