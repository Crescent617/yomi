use super::*;

use crate::types::{SessionId, ToolCall};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn test_permission_checker_auto_approve() {
    let bus = crate::comms::EventBus::new();
    let handle = bus.handle(SessionId::new());
    let input_bus = crate::comms::InputBus::new();

    // Caution 阈值 - Safe 工具应该自动通过
    let state = PermissionState::new(Level::Caution);
    let checker = Checker::new(state, handle, input_bus, SessionId::new());

    let safe_tool = ToolCall {
        id: "test1".to_string(),
        name: "Read".to_string(),
        arguments: json!({}),
    };

    // Safe 工具应该自动批准（不需要发送事件）
    assert!(checker
        .check_permission(&safe_tool, Level::Safe)
        .await
        .unwrap());
}

#[tokio::test]
async fn test_permission_via_input_bus() {
    let bus = crate::comms::EventBus::new();
    let handle = bus.handle(SessionId::new());
    let input_bus = crate::comms::InputBus::new();
    let session_id = SessionId::new();

    let state = PermissionState::new(Level::Safe); // Safe 级别，Caution 工具需要确认
    let checker = Checker::new(
        state,
        handle.clone(),
        Arc::clone(&input_bus),
        session_id.clone(),
    );

    let caution_tool = ToolCall {
        id: "test1".to_string(),
        name: "Edit".to_string(),
        arguments: json!({"file_path": "test.txt"}),
    };

    let checker_task = tokio::spawn(async move {
        checker
            .check_permission(&caution_tool, Level::Caution)
            .await
    });

    // 接收权限请求事件
    let mut sub = bus.subscribe_all();
    let envelope = sub.recv().await.unwrap().1;
    let event = envelope.event;
    let req_id = match event {
        Event::Agent(AgentEvent::PermissionRequest { req_id, .. }) => req_id,
        _ => panic!("Expected PermissionRequest event"),
    };

    // 通过 input_bus 发送响应（模拟 TUI 行为）
    let _ = input_bus.publish(
        session_id.clone(),
        AgentInput::PermissionResponse {
            req_id: req_id.clone(),
            approved: true,
            remember: false,
        },
    );

    let check_result = checker_task.await.unwrap().unwrap();
    assert!(check_result);
}

#[tokio::test]
async fn test_permission_remember_per_tool() {
    let bus = crate::comms::EventBus::new();
    let handle = bus.handle(SessionId::new());
    let input_bus = crate::comms::InputBus::new();
    let session_id = SessionId::new();

    let state = PermissionState::new(Level::Safe);
    let checker = Checker::new(
        state,
        handle.clone(),
        Arc::clone(&input_bus),
        session_id.clone(),
    );
    let checker = Arc::new(checker);

    let edit_tool = ToolCall {
        id: "test1".to_string(),
        name: "Edit".to_string(),
        arguments: json!({"file_path": "test.txt"}),
    };

    let checker_clone = Arc::clone(&checker);
    let checker_task = tokio::spawn(async move {
        checker_clone
            .check_permission(&edit_tool, Level::Caution)
            .await
    });

    let mut sub = bus.subscribe_all();
    let envelope = sub.recv().await.unwrap().1;
    let event = envelope.event;
    let req_id = match event {
        Event::Agent(AgentEvent::PermissionRequest { req_id, .. }) => req_id,
        _ => panic!("Expected PermissionRequest event"),
    };

    // 通过 input_bus 发送响应，选择 remember
    let _ = input_bus.publish(
        session_id.clone(),
        AgentInput::PermissionResponse {
            req_id: req_id.clone(),
            approved: true,
            remember: true,
        },
    );

    let result = checker_task.await.unwrap().unwrap();
    assert!(result);

    // Drain the PermissionAck from the first request
    let _ack = sub.recv().await;

    // 再次请求 Edit 工具，应该自动通过（不需要事件）
    let edit_tool2 = ToolCall {
        id: "test2".to_string(),
        name: "Edit".to_string(),
        arguments: json!({"file_path": "test2.txt"}),
    };

    let result = checker
        .check_permission(&edit_tool2, Level::Caution)
        .await
        .unwrap();
    assert!(result);

    // 没有事件发送（因为 Edit 已自动批准）
    let timeout_result =
        tokio::time::timeout(std::time::Duration::from_millis(100), sub.recv()).await;
    assert!(
        timeout_result.is_err(),
        "Should not receive event for auto-approved Edit tool"
    );
}
