use super::strip_bot_mention;
use serde_json::json;

#[test]
fn strip_bot_mention_preserves_other_mentions() {
    let mentions = vec![
        json!({ "key": "@_user_1", "id": { "open_id": "bot-id" } }),
        json!({ "key": "@_user_2", "id": { "open_id": "alice-id" } }),
    ];

    let raw_text = strip_bot_mention(
        "@_user_1 /steer ask @_user_2 to inspect logs",
        Some(&mentions),
        Some("bot-id"),
    );

    assert_eq!(raw_text, "/steer ask @_user_2 to inspect logs");
}

#[test]
fn strip_bot_mention_keeps_text_when_bot_id_is_unknown() {
    let mentions = vec![json!({
        "key": "@_user_1",
        "id": { "open_id": "someone-else" }
    })];

    let raw_text = strip_bot_mention("@_user_1 hello", Some(&mentions), None);

    assert_eq!(raw_text, "@_user_1 hello");
}
