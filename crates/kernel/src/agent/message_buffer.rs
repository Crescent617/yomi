use crate::types::Message;
use std::sync::Arc;

/// Simple message buffer for agent conversation history
#[derive(Debug, Clone)]
pub struct MessageBuffer {
    messages: Vec<Arc<Message>>,
}

impl Default for MessageBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl MessageBuffer {
    /// Create an empty buffer
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Create from existing messages (for recovery)
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self {
            messages: messages.into_iter().map(Arc::new).collect(),
        }
    }

    /// Create from existing Arc messages (internal use)
    pub fn from_arc_messages(messages: &[Arc<Message>]) -> Self {
        Self {
            messages: messages.to_vec(),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(Arc::new(message));
    }

    /// Push an already-arc-wrapped message
    pub fn push_arc(&mut self, message: Arc<Message>) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[Arc<Message>] {
        &self.messages
    }

    /// Get mutable access to the underlying vector (use with caution)
    pub fn messages_mut(&mut self) -> &mut Vec<Arc<Message>> {
        &mut self.messages
    }

    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Update a message using Copy-on-Write pattern
    /// If the Arc is shared, it will be cloned before modification
    pub fn update_message<F>(&mut self, idx: usize, f: F)
    where
        F: FnOnce(&mut Message),
    {
        if let Some(arc) = self.messages.get_mut(idx) {
            // Arc::make_mut will clone the inner data if it's shared
            let message = Arc::make_mut(arc);
            f(message);
        }
    }

    /// Get a clone of the messages as a new Vec<Arc<Message>>
    pub fn clone_messages(&self) -> Vec<Arc<Message>> {
        self.messages.clone()
    }

    /// Return the provider-facing message view: internal metadata removed and
    /// incomplete assistant/tool groups sanitized without mutating stored history.
    pub fn sanitized_model_messages(messages: &[Arc<Message>]) -> Vec<Arc<Message>> {
        let mut buffer = Self {
            messages: messages
                .iter()
                .filter(|message| message.role != crate::types::Role::Internal)
                .cloned()
                .collect(),
        };
        buffer.sanitize();
        buffer.messages
    }

    /// Sanitize the message buffer by removing inconsistent tool call/response pairs.
    /// Removes assistant messages with `tool_calls` that don't have corresponding tool responses,
    /// and removes tool responses that are not immediately after their corresponding assistant.
    /// Also removes empty assistant messages (no content, no tool calls) — poison
    /// persisted by a model hiccup (empty completion); replaying them makes strict
    /// gateways 400 every request. Dropping them here lets already-poisoned
    /// sessions self-heal on the next turn.
    /// Time: O(n), Space: O(k) where k = number of pending tool calls
    pub fn sanitize(&mut self) {
        use crate::types::Role;
        use std::collections::HashSet;

        // First pass: find all valid (assistant -> tool chain) groups
        // A tool response is valid only if it immediately follows its assistant
        let mut to_remove = HashSet::new();
        let n = self.messages.len();
        let mut i = 0;
        let mut expected_tool_ids = HashSet::new();
        let mut tool_msg_indices = Vec::new();

        while i < n {
            let msg = &self.messages[i];

            // Non-assistant: Tool gets marked, others skipped
            let Role::Assistant = msg.role else {
                if msg.role == Role::Tool {
                    to_remove.insert(i);
                }
                i += 1;
                continue;
            };

            // Assistant without tool_calls: keep it unless it carries no
            // content at all — that shape is empty-completion poison.
            let Some(calls) = msg.tool_calls.as_ref() else {
                if msg.content.is_empty() {
                    to_remove.insert(i);
                }
                i += 1;
                continue;
            };

            expected_tool_ids.clear();
            tool_msg_indices.clear();

            for call in calls {
                expected_tool_ids.insert(call.id.clone());
            }

            let tool_call_count = calls.len();
            let mut valid_chain = true;

            for tool_idx in i + 1..=i + tool_call_count {
                let Some(tool_msg) = self.messages.get(tool_idx) else {
                    valid_chain = false;
                    break;
                };

                if tool_msg.role != Role::Tool {
                    valid_chain = false;
                    break;
                }

                tool_msg_indices.push(tool_idx);

                let Some(ref tool_call_id) = tool_msg.tool_call_id else {
                    valid_chain = false;
                    break;
                };

                if !expected_tool_ids.remove(tool_call_id) {
                    valid_chain = false;
                    break;
                }
            }

            // Check if all expected tool calls have responses
            if valid_chain && !expected_tool_ids.is_empty() {
                valid_chain = false;
            }

            if !valid_chain {
                to_remove.insert(i);
                to_remove.extend(tool_msg_indices.iter());
                i += 1 + tool_msg_indices.len();
                continue;
            }

            // Valid chain - skip past all tool responses
            i += tool_call_count + 1;
        }

        if to_remove.is_empty() {
            return;
        }

        let mut i = 0;
        self.messages.retain(|_| {
            let keep = !to_remove.contains(&i);
            i += 1;
            keep
        });
    }
}

#[cfg(test)]
#[path = "message_buffer_test.rs"]
mod tests;
