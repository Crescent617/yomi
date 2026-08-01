use super::pet_activity;
use crate::event::{AgentEvent, AgentStatus};
use crate::notification::AgentActivity;

#[test]
fn pet_activity_only_summarizes_relevant_events() {
    let permission = AgentEvent::PermissionRequest {
        req_id: "request-1".into(),
        session_id: "session-1".into(),
        tool_id: "tool-1".into(),
        tool_name: "shell".into(),
        tool_args: "secret args".into(),
        tool_level: "caution".into(),
        reason: "secret reason".into(),
    };

    assert_eq!(
        pet_activity(&permission),
        Some(AgentActivity::PermissionRequested {
            req_id: "request-1".into(),
            target_session_id: "session-1".into(),
        })
    );
    assert_eq!(
        pet_activity(&AgentEvent::Lifecycle {
            state: AgentStatus::Running,
        }),
        Some(AgentActivity::Started)
    );
    assert_eq!(
        pet_activity(&AgentEvent::Retrying {
            attempt: 1,
            max_attempts: 3,
            reason: "not globally forwarded".into(),
            wait_ms: 0,
        }),
        None
    );
}
