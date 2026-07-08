use super::*;

use crate::storage::todo::JsonTodoStore;
use tempfile::TempDir;

async fn make_store_with_todos(temp: &TempDir, json: &str) -> Arc<dyn TodoStore> {
    let store: Arc<dyn TodoStore> = Arc::new(JsonTodoStore::new(temp.path()));
    store.save("session-1", json).await.unwrap();
    store
}

fn empty_history() -> Vec<Arc<Message>> {
    Vec::new()
}

fn history_with_user_msgs(count: usize) -> Vec<Arc<Message>> {
    (0..count).map(|_| Arc::new(Message::user("hi"))).collect()
}

fn history_with_todo_then_users(user_count: usize) -> Vec<Arc<Message>> {
    let mut msgs = Vec::new();
    // assistant message with a todoWrite tool call
    msgs.push(Arc::new(Message {
        role: Role::Assistant,
        content: vec![],
        tool_calls: Some(vec![crate::types::ToolCall {
            id: "call_1".into(),
            name: crate::tools::TODO_TOOL_NAME.into(),
            arguments: serde_json::json!({}),
        }]),
        tool_call_id: None,
        created_at: chrono::Utc::now(),
        token_usage: None,
        response_id: None,
        finish_reason: None,
        ..Default::default()
    }));
    for _ in 0..user_count {
        msgs.push(Arc::new(Message::user("hi")));
    }
    msgs
}

#[tokio::test]
async fn test_interval_zero_does_not_trigger() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"pending"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store).with_interval(0);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    interceptor
        .intercept(&mut content, &ctx(&empty_history()))
        .await;
    assert_eq!(content.len(), 1);
    assert_eq!(extract_text(&content), "hello");
}

#[tokio::test]
async fn test_triggers_on_interval() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"pending"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store).with_interval(5);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];

    // 4 user msgs since last todo → current is 5th → trigger
    let history = history_with_todo_then_users(4);
    interceptor.intercept(&mut content, &ctx(&history)).await;
    let text = extract_text(&content);
    assert!(text.contains("pending todos"));
    assert!(text.contains("1. (pending) A"));
    assert!(!text.contains("[pending]"));
}

#[tokio::test]
async fn test_does_not_trigger_when_not_yet_interval() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"pending"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store).with_interval(5);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];

    // 3 user msgs since last todo → current is 4th → no trigger
    let history = history_with_todo_then_users(3);
    interceptor.intercept(&mut content, &ctx(&history)).await;
    assert_eq!(extract_text(&content), "hello");
}

#[tokio::test]
async fn test_no_todos_does_nothing() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(&temp, r#"{"todos":[]}"#).await;
    let interceptor = TodoReminderInterceptor::new(store);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    let history = history_with_user_msgs(4);
    interceptor.intercept(&mut content, &ctx(&history)).await;
    assert_eq!(extract_text(&content), "hello");
}

#[tokio::test]
async fn test_only_completed_todos_does_nothing() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"completed"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    let history = history_with_user_msgs(4);
    interceptor.intercept(&mut content, &ctx(&history)).await;
    assert_eq!(extract_text(&content), "hello");
}

#[tokio::test]
async fn test_completed_todos_filtered() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
            &temp,
            r#"{"todos":[{"id":"1","content":"Done","status":"completed"},{"id":"2","content":"B","status":"in_progress"}]}"#,
        )
        .await;
    let interceptor = TodoReminderInterceptor::new(store);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    // default interval = 3; 2 user msgs → current = 3rd → trigger
    let history = history_with_user_msgs(2);
    interceptor.intercept(&mut content, &ctx(&history)).await;
    let text = extract_text(&content);
    assert!(text.contains('B'));
    assert!(!text.contains("Done"));
}

#[tokio::test]
async fn test_reminder_appended_to_last_text_block() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"pending"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    // default interval = 3; 2 user msgs → current = 3rd → trigger
    let history = history_with_user_msgs(2);
    interceptor.intercept(&mut content, &ctx(&history)).await;
    assert_eq!(content.len(), 1);
    let text = extract_text(&content);
    assert!(text.starts_with("hello"));
    assert!(text.contains("pending todos"));
}

#[tokio::test]
async fn test_reminder_creates_new_block_when_no_text() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"pending"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store);
    let mut content: Vec<ContentBlock> = vec![];
    // default interval = 3; 2 user msgs → current = 3rd → trigger
    let history = history_with_user_msgs(2);
    interceptor.intercept(&mut content, &ctx(&history)).await;
    assert_eq!(content.len(), 1);
    assert!(extract_text(&content).contains("pending todos"));
}

#[tokio::test]
async fn test_system_reminder_tags_present() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"pending"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    // default interval = 3; 2 user msgs → current = 3rd → trigger
    let history = history_with_user_msgs(2);
    interceptor.intercept(&mut content, &ctx(&history)).await;
    let text = extract_text(&content);
    assert!(text.contains(SYSTEM_REMINDER_START));
    assert!(text.contains(SYSTEM_REMINDER_END));
}

#[tokio::test]
async fn test_resets_interval_after_todo_tool() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"pending"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store).with_interval(5);

    // 4 user msgs after todoWrite → current is 5th → trigger
    let history = history_with_todo_then_users(4);
    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    interceptor.intercept(&mut content, &ctx(&history)).await;
    assert!(extract_text(&content).contains("pending todos"));

    // Reset: add another todo tool + 3 user msgs → current is 4th → no trigger
    let mut history2 = history.clone();
    history2.push(Arc::new(Message {
        role: Role::Assistant,
        content: vec![],
        tool_calls: Some(vec![crate::types::ToolCall {
            id: "call_2".into(),
            name: crate::tools::TODO_TOOL_NAME.into(),
            arguments: serde_json::json!({}),
        }]),
        tool_call_id: None,
        created_at: chrono::Utc::now(),
        token_usage: None,
        response_id: None,
        finish_reason: None,
        ..Default::default()
    }));
    for _ in 0..3 {
        history2.push(Arc::new(Message::user("hi")));
    }
    let mut content2 = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    interceptor.intercept(&mut content2, &ctx(&history2)).await;
    assert_eq!(extract_text(&content2), "hello");

    // Add 1 more user msg → current is 5th since last todo → trigger
    history2.push(Arc::new(Message::user("hi")));
    let mut content3 = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    interceptor.intercept(&mut content3, &ctx(&history2)).await;
    assert!(extract_text(&content3).contains("pending todos"));
}

#[tokio::test]
async fn test_non_todo_tool_does_not_reset() {
    let temp = TempDir::new().unwrap();
    let store = make_store_with_todos(
        &temp,
        r#"{"todos":[{"id":"1","content":"A","status":"pending"}]}"#,
    )
    .await;
    let interceptor = TodoReminderInterceptor::new(store).with_interval(5);

    // read tool (not todo) + 4 user msgs → current is 5th since last todo → trigger
    let mut history = Vec::new();
    history.push(Arc::new(Message {
        role: Role::Assistant,
        content: vec![],
        tool_calls: Some(vec![crate::types::ToolCall {
            id: "call_1".into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
        }]),
        tool_call_id: None,
        created_at: chrono::Utc::now(),
        token_usage: None,
        response_id: None,
        finish_reason: None,
        ..Default::default()
    }));
    for _ in 0..4 {
        history.push(Arc::new(Message::user("hi")));
    }

    let mut content = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    interceptor.intercept(&mut content, &ctx(&history)).await;
    assert!(extract_text(&content).contains("pending todos"));
}

fn extract_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn ctx(history: &[Arc<Message>]) -> InterceptCtx<'_> {
    InterceptCtx {
        session_id: "session-1",
        history,
    }
}
