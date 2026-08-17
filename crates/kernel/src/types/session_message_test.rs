use super::{SessionMessage, UserMsg};
use crate::types::{Message, MessageTokenUsage, IS_STEER_META_KEY};
use std::collections::HashMap;

#[test]
fn from_storage_classifies_marked_user_message_as_steer() {
    let mut message = Message::user("redirect");
    message.metadata = Some(HashMap::from([(
        IS_STEER_META_KEY.to_string(),
        "true".to_string(),
    )]));

    let messages = SessionMessage::from_storage(vec![message]);

    assert!(matches!(messages.as_slice(), [SessionMessage::Steer(_)]));
}

#[test]
fn from_storage_keeps_unmarked_user_message_as_user() {
    let messages = SessionMessage::from_storage(vec![Message::user("hello")]);

    assert!(matches!(messages.as_slice(), [SessionMessage::User(_)]));
}

#[test]
fn steer_serializes_with_snake_case_kind() {
    let message = Message::user("redirect");
    let steer = SessionMessage::Steer(UserMsg {
        id: message.id,
        content: message.content,
        created_at: message.created_at,
    });

    let value = serde_json::to_value(steer).expect("steer should serialize");

    assert_eq!(value["kind"], "steer");
}

#[test]
fn from_storage_maps_assistant_model_id() {
    let message = Message::assistant("hi")
        .with_model_id("gpt-5.2")
        .with_response_id("chatcmpl-123")
        .with_token_usage(MessageTokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        });

    let messages = SessionMessage::from_storage(vec![message]);

    let [SessionMessage::Assistant(msg)] = messages.as_slice() else {
        panic!("expected a single assistant message");
    };
    assert_eq!(msg.model_id.as_deref(), Some("gpt-5.2"));
    assert_eq!(msg.response_id.as_deref(), Some("chatcmpl-123"));
}

#[test]
fn assistant_message_jsonl_roundtrip_keeps_model_id() {
    let message = Message::assistant("hi").with_model_id("claude-sonnet-4");

    let line = serde_json::to_string(&message).expect("serialize");
    let parsed: Message = serde_json::from_str(&line).expect("deserialize");

    assert_eq!(parsed.model_id.as_deref(), Some("claude-sonnet-4"));
}

#[test]
fn legacy_message_without_model_id_deserializes() {
    let line =
        r#"{"id":"msg_1","role":"assistant","content":"hi","created_at":"2026-01-01T00:00:00Z"}"#;

    let parsed: Message = serde_json::from_str(line).expect("deserialize legacy message");

    assert_eq!(parsed.model_id, None);
}

#[test]
fn from_storage_classifies_marked_user_message_as_interrupted() {
    let mut message = Message::user("[interrupted: cancelled]");
    message.metadata = Some(HashMap::from([(
        crate::types::INTERRUPTED_META_KEY.to_string(),
        "true".to_string(),
    )]));

    let messages = SessionMessage::from_storage(vec![message]);

    assert!(matches!(
        messages.as_slice(),
        [SessionMessage::Interrupted(_)]
    ));
}
