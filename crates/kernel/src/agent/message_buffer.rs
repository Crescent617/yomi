use crate::types::Message;

/// 管理消息历史，防止无限增长
#[derive(Debug, Clone)]
pub struct MessageBuffer {
    messages: Vec<Message>,
    max_messages: usize,
    summary_threshold: usize,
}

impl MessageBuffer {
    pub const fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_messages,
            summary_threshold: max_messages.saturating_sub(10),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);

        if self.messages.len() > self.max_messages {
            self.truncate_oldest();
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[allow(dead_code)]
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }

    fn truncate_oldest(&mut self) {
        // 保留系统消息和最近的消息
        let system_count = self
            .messages
            .iter()
            .take_while(|m| matches!(m.role, crate::types::Role::System))
            .count();

        // 移除最早的用户/助手消息，但保留系统提示
        let remove_count = (self.messages.len() - self.summary_threshold).max(0);
        if remove_count > 0 && system_count < self.messages.len() {
            let start_idx = system_count + 1; // 保留系统消息后第一条
            let end_idx = (start_idx + remove_count).min(self.messages.len());

            tracing::info!("Truncating messages {}..{}", start_idx, end_idx);

            // 简化的截断：直接删除旧消息
            // 更复杂的实现可以生成摘要
            self.messages.drain(start_idx..end_idx);

            // 添加摘要提示
            self.messages.insert(
                system_count + 1,
                Message::system("(Earlier conversation history has been truncated due to length)"),
            );
        }
    }

    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    #[allow(dead_code)]
    pub const fn is_empty(&self) -> bool {
        self.messages.is_empty()
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
    fn test_buffer_truncates_when_full() {
        let mut buffer = MessageBuffer::new(5);

        buffer.push(create_message(Role::System, "System prompt"));
        buffer.push(create_message(Role::User, "Message 1"));
        buffer.push(create_message(Role::Assistant, "Response 1"));
        buffer.push(create_message(Role::User, "Message 2"));
        buffer.push(create_message(Role::Assistant, "Response 2"));
        buffer.push(create_message(Role::User, "Message 3"));

        assert!(buffer.len() <= 5);
    }

    #[test]
    fn test_preserves_system_message() {
        let mut buffer = MessageBuffer::new(3);

        buffer.push(create_message(Role::System, "System prompt"));
        buffer.push(create_message(Role::User, "User message"));
        buffer.push(create_message(Role::Assistant, "Assistant response"));
        buffer.push(create_message(Role::User, "Another user"));

        // 系统消息应该保留
        assert!(buffer
            .messages()
            .iter()
            .any(|m| { matches!(m.role, Role::System) }));
    }
}
