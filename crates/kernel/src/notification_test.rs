use serde_json::json;

use super::{AgentActivity, Notification};
use crate::types::SessionId;

#[test]
fn agent_activity_uses_snake_case_wire_shape() {
    let notification = Notification::AgentActivity {
        session_id: SessionId::from("session-1".to_string()),
        event_id: "event-1".into(),
        activity: AgentActivity::PermissionRequested {
            req_id: "request-1".into(),
            target_session_id: "session-1".into(),
        },
    };

    assert_eq!(
        serde_json::to_value(notification).unwrap(),
        json!({
            "agent_activity": {
                "session_id": "session-1",
                "event_id": "event-1",
                "activity": {
                    "kind": "permission_requested",
                    "req_id": "request-1",
                    "target_session_id": "session-1"
                }
            }
        })
    );
}
