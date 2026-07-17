use super::{ProjectId, RunningSessionResponse, SessionId, SubagentResponse};

#[test]
fn running_session_response_serializes_frontend_contract() {
    let response = RunningSessionResponse {
        id: SessionId::from("sub_child"),
        parent_id: Some(SessionId::from("ses_parent")),
        title: Some("Research".to_string()),
        project_id: Some(ProjectId::from("project_1")),
        phase: "executing_tool".to_string(),
        background_task_count: 1,
        background_shells: vec![crate::agent::BackgroundShellTask {
            task_id: "sh-1".to_string(),
            session_id: SessionId::from("sub_child"),
            pid: 42,
            command: "cargo test".to_string(),
            output_path: "/tmp/yomi_sh-1.log".to_string(),
            started_at: chrono::DateTime::parse_from_rfc3339("2026-07-12T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        }],
    };

    let value = serde_json::to_value(response).unwrap();

    assert_eq!(value["id"], "sub_child");
    assert_eq!(value["parent_id"], "ses_parent");
    assert_eq!(value["title"], "Research");
    assert_eq!(value["project_id"], "project_1");
    assert_eq!(value["phase"], "executing_tool");
    assert_eq!(value["background_task_count"], 1);
    assert_eq!(value["background_shells"][0]["task_id"], "sh-1");
    assert_eq!(value["background_shells"][0]["pid"], 42);
    assert_eq!(value["background_shells"][0]["command"], "cargo test");
}

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
