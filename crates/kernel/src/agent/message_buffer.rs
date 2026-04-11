use crate::types::Message;

/// Simple message buffer for agent conversation history
#[derive(Debug, Clone)]
pub struct MessageBuffer {
    messages: Vec<Message>,
}

impl MessageBuffer {
    /// Create from existing messages (for recovery)
    pub const fn from_messages(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub const fn len(&self) -> usize {
        self.messages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, Role};

    fn create_message(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            tool_calls: None,
            tool_call_id: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_buffer_basic() {
        let mut buffer = MessageBuffer {
            messages: Vec::new(),
        };

        buffer.push(create_message(Role::System, "System prompt"));
        buffer.push(create_message(Role::User, "Message 1"));
        buffer.push(create_message(Role::Assistant, "Response 1"));

        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn test_from_messages() {
        let messages = vec![
            create_message(Role::System, "System"),
            create_message(Role::User, "User"),
        ];
        let buffer = MessageBuffer::from_messages(messages);

        assert_eq!(buffer.len(), 2);
        assert!(buffer
            .messages()
            .iter()
            .any(|m| { matches!(m.role, Role::System) }));
    }
}
