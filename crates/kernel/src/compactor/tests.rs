use super::*;

use crate::types::{MessageId, MessageTokenUsage};
use std::sync::Arc;

#[test]
fn test_calculate_tokens_with_usage() {
    let messages: Vec<Arc<Message>> = vec![
        Arc::new(Message::user("Hello")),
        Arc::new(Message::assistant("Hi there")),
        {
            let mut msg = Message::assistant("Let me help");
            msg.token_usage = Some(MessageTokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            });
            Arc::new(msg)
        },
    ];

    let tokens = Compactor::calculate_tokens(&messages);
    // Should use the actual usage (150) plus estimation for messages after
    assert!(tokens >= 150);
}

#[test]
fn test_micro_compact() {
    use std::sync::Arc;

    let compactor = Compactor::new(0.5, 200, 2, 1000); // threshold=100, keep last 2 messages
    let messages: Vec<Arc<Message>> = vec![
        Arc::new(Message::user("Task 1")),
        Arc::new(Message::tool_result(
            MessageId::default(),
            "call-1",
            "Result 1",
        )), // will be cleared (index 1)
        Arc::new(Message::user("Task 2")),
        Arc::new(Message::tool_result(
            MessageId::default(),
            "call-2",
            "Result 2",
        )), // kept (index 3, in keep_recent)
        Arc::new(Message::user("Current task")), // kept (index 4)
    ];

    let compacted = compactor.micro_compact(&messages);
    assert!(compacted.is_some());
    let new_messages = compacted.unwrap();
    // Old tool result should be cleared
    assert_eq!(
        new_messages[1].text_content(),
        "[Old tool result content cleared]"
    );
    // Recent tool result should be preserved (keep_recent = 2)
    assert_eq!(new_messages[3].text_content(), "Result 2");
    assert_eq!(new_messages[4].text_content(), "Current task");

    // Second compaction should return None (already cleared)
    let compacted_again = compactor.micro_compact(&new_messages);
    assert!(compacted_again.is_none());
}
