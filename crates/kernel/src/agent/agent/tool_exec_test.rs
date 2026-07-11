use super::{assign_message_ids, run_single_tool, RunSingleToolParams};
use crate::event::ToolEvent;
use crate::types::ToolCall;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn preassigned_message_id_is_preserved_by_tool_result() {
    let call = ToolCall {
        id: "tool-call".to_string(),
        name: "missing-tool".to_string(),
        arguments: json!({}),
    };
    let message_ids = assign_message_ids(std::slice::from_ref(&call));
    let message_id = message_ids[&call.id].clone();

    let result = run_single_tool(RunSingleToolParams {
        tool_opt: None,
        call_id: &call.id,
        call_name: &call.name,
        arguments: call.arguments,
        message_id: message_id.clone(),
        cancel_token: CancellationToken::new(),
        working_dir: std::path::PathBuf::from("."),
        session_id: "session".to_string(),
        turn: None,
        max_tool_output_length: 1024,
    })
    .await;

    assert_eq!(result.message_id, message_id);
    assert_eq!(result.message.id, message_id);
    assert_eq!(
        result.message.tool_call_id.as_deref(),
        Some(call.id.as_str())
    );
    match result.event {
        ToolEvent::End {
            message_id: event_message_id,
            tool_id,
            ..
        } => {
            assert_eq!(event_message_id, message_id);
            assert_eq!(tool_id, call.id);
        }
        event => panic!("expected tool end event, got {event:?}"),
    }
}
