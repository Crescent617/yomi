use super::{SessionId, SubagentResponse};

#[test]
fn subagent_response_serializes_frontend_contract() {
    let response = SubagentResponse {
        id: SessionId::from("sub_child"),
        alias: Some("Research".to_string()),
        parent_session_id: SessionId::from("ses_parent"),
        phase: "executing_tool".to_string(),
        is_running: true,
        model_key: Some("default".to_string()),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-07-12T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    };

    let value = serde_json::to_value(response).unwrap();

    assert_eq!(value["id"], "sub_child");
    assert_eq!(value["alias"], "Research");
    assert_eq!(value["parent_session_id"], "ses_parent");
    assert_eq!(value["phase"], "executing_tool");
    assert_eq!(value["is_running"], true);
    assert_eq!(value["model_key"], "default");
    assert_eq!(value["created_at"], "2026-07-12T12:00:00Z");
}
