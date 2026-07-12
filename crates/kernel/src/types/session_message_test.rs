use super::{SessionMessage, UserMsg};
use crate::types::{Message, IS_STEER_META_KEY};
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
