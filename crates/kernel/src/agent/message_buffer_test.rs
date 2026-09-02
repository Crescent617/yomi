use super::*;
use crate::types::{ContentBlock, Message, Role, ToolCall};
use chrono::Utc;

fn create_assistant_with_tools(tool_ids: Vec<&str>) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "calling tools".to_string(),
        }],
        tool_calls: Some(
            tool_ids
                .into_iter()
                .map(|tid| ToolCall {
                    id: tid.to_string(),
                    name: "test_tool".to_string(),
                    arguments: serde_json::json!({}),
                })
                .collect(),
        ),
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    }
}

fn create_tool_response(tool_call_id: &str) -> Message {
    Message {
        role: Role::Tool,
        content: vec![ContentBlock::Text {
            text: "result".to_string(),
        }],
        tool_calls: None,
        tool_call_id: Some(tool_call_id.to_string()),
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    }
}

fn create_user_message(content: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: content.to_string(),
        }],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    }
}

#[test]
fn test_valid_chain_kept() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.messages()[0].role, Role::Assistant);
    assert_eq!(buffer.messages()[1].role, Role::Tool);
}

#[test]
fn test_multiple_tools_kept() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1", "t2"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_tool_response("t2"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 3);
}

#[test]
fn test_interrupted_chain_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_user_message("interrupt"));
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 1);
    assert_eq!(buffer.messages()[0].role, Role::User);
}

#[test]
fn test_orphan_tool_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 0);
}

#[test]
fn test_missing_tool_response_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1", "t2"]));
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 0);
}

#[test]
fn test_extra_tool_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_tool_response("extra"));

    buffer.sanitize();

    // Only the orphan extra tool is removed, valid chain is kept
    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.messages()[0].role, Role::Assistant);
    assert_eq!(buffer.messages()[1].role, Role::Tool);
}

#[test]
fn test_wrong_tool_id_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t2"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 0);
}

#[test]
fn test_multiple_valid_chains() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_assistant_with_tools(vec!["t2"]));
    buffer.push(create_tool_response("t2"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 4);
}

#[test]
fn test_mixed_chains() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_assistant_with_tools(vec!["t2"]));
    buffer.push(create_user_message("interrupt"));
    buffer.push(create_tool_response("t2"));
    buffer.push(create_tool_response("orphan"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 3);
    assert_eq!(buffer.messages()[0].role, Role::Assistant);
    assert_eq!(buffer.messages()[1].role, Role::Tool);
    assert_eq!(buffer.messages()[2].role, Role::User);
}

#[test]
fn test_empty_buffer() {
    let mut buffer = MessageBuffer::new();
    buffer.sanitize();
    assert_eq!(buffer.len(), 0);
}

#[test]
fn test_assistant_without_tools() {
    let mut buffer = MessageBuffer::new();
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
        }],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });
    buffer.push(create_user_message("response"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 2);
}

#[test]
fn test_duplicate_tool_response_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    // Only the duplicate tool response is removed, valid chain is kept
    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.messages()[0].role, Role::Assistant);
    assert_eq!(buffer.messages()[1].role, Role::Tool);
}

/// 空 completion 毒化自愈：无内容、无 tool_calls 的 assistant 消息（模型
/// 抽风落盘的毒）在 sanitize 时被摘除，其余消息原样保留——已中毒 session
/// 升级后下一轮自动康复，不必手工删 jsonl。
#[test]
fn test_empty_assistant_poison_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_user_message("before"));
    // The poison shape: content == [], tool_calls == None (metadata-only
    // assistant persisted by the pre-fix guard).
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });
    buffer.push(create_user_message("after"));

    buffer.sanitize();

    let roles: Vec<_> = buffer.messages().iter().map(|m| m.role).collect();
    assert_eq!(roles, vec![Role::User, Role::User]);
}

#[test]
fn test_nonempty_assistant_without_tools_kept() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_user_message("hi"));
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "answer".to_string(),
        }],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });

    buffer.sanitize();

    assert_eq!(buffer.messages().len(), 2);
    assert_eq!(buffer.messages()[1].role, Role::Assistant);
}

/// 边界 pin：仅含 thinking 的 assistant（content 非空、无 tool_calls）不是
/// 毒，sanitize 必须保留——防止未来"清理空消息"类重构把规则放宽误伤。
#[test]
fn test_thinking_only_assistant_kept() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_user_message("question"));
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Thinking {
            thinking: "reasoning".to_string(),
            signature: Some("sig".to_string()),
        }],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });

    buffer.sanitize();

    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.messages()[1].role, Role::Assistant);
}

/// 新规则与链式清理同轮交互：[带调用的 assistant, 空毒消息, 孤儿 tool]
/// 三者全部摘除——空毒使前一条链断裂，tool 本就成为孤儿。
#[test]
fn test_poison_between_chain_and_tool_removes_all() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 0);
}
