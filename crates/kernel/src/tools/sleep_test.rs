use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

use super::{should_wake, SleepTool};
use crate::agent::AgentInput;
use crate::comms::InputBus;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{ContentBlock, SessionId};

fn text_of(output: &crate::types::ToolOutput) -> String {
    output.contents.iter().filter_map(|b| b.as_text()).collect()
}

fn ctx(session_id: &str) -> ToolExecCtx<'static> {
    ToolExecCtx::new("call_1", "/tmp", session_id)
}

fn user_input() -> AgentInput {
    AgentInput::User {
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
        }],
    }
}

#[tokio::test]
async fn test_definition() {
    let tool = SleepTool::new(None);

    assert_eq!(tool.name(), "sleep");
    assert!(tool.desc().contains("Wakes up early"));
    assert_eq!(tool.schema()["required"], json!(["seconds"]));
}

#[tokio::test]
async fn test_validation() {
    let tool = SleepTool::new(None);

    assert!(tool.exec(json!({}), ctx("s1")).await.is_err());
    assert!(tool.exec(json!({"seconds": "5"}), ctx("s1")).await.is_err());
    assert!(tool
        .exec(json!({"seconds": 3601}), ctx("s1"))
        .await
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn test_sleeps_full_duration_without_bus() {
    let tool = SleepTool::new(None);

    let output = tool.exec(json!({"seconds": 5}), ctx("s1")).await.unwrap();

    assert_eq!(text_of(&output), "Slept for 5 seconds");
}

#[tokio::test(start_paused = true)]
async fn test_sleeps_full_duration_with_quiet_bus() {
    let bus = InputBus::new();
    let tool = SleepTool::new(Some(bus));

    let output = tool.exec(json!({"seconds": 5}), ctx("s1")).await.unwrap();

    assert_eq!(text_of(&output), "Slept for 5 seconds");
}

#[tokio::test(start_paused = true)]
async fn test_does_not_wake_on_user_input() {
    let bus = InputBus::new();
    let tool = SleepTool::new(Some(Arc::clone(&bus)));

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        bus.publish(SessionId::from("s1"), user_input()).unwrap();
    });

    let output = tool.exec(json!({"seconds": 60}), ctx("s1")).await.unwrap();

    assert_eq!(text_of(&output), "Slept for 60 seconds");
}

#[tokio::test(start_paused = true)]
async fn test_wakes_early_on_steer() {
    let bus = InputBus::new();
    let tool = SleepTool::new(Some(Arc::clone(&bus)));

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        bus.publish(
            SessionId::from("s1"),
            AgentInput::Steer(vec![ContentBlock::Text {
                text: "background task done".to_string(),
            }]),
        )
        .unwrap();
    });

    let output = tool.exec(json!({"seconds": 60}), ctx("s1")).await.unwrap();

    let text = text_of(&output);
    assert!(
        text.contains("Sleep interrupted after 3 seconds (planned 60 seconds)"),
        "unexpected output: {text}"
    );
    assert!(text.contains("steer message"), "unexpected output: {text}");
}

#[tokio::test(start_paused = true)]
async fn test_ignores_other_sessions() {
    let bus = InputBus::new();
    let tool = SleepTool::new(Some(Arc::clone(&bus)));

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        bus.publish(SessionId::from("other_session"), AgentInput::Steer(vec![]))
            .unwrap();
    });

    let output = tool.exec(json!({"seconds": 5}), ctx("s1")).await.unwrap();

    assert_eq!(text_of(&output), "Slept for 5 seconds");
}

#[tokio::test(start_paused = true)]
async fn test_does_not_wake_on_non_steer_inputs() {
    let bus = InputBus::new();
    let tool = SleepTool::new(Some(Arc::clone(&bus)));

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        for input in [
            user_input(),
            AgentInput::Cancel,
            AgentInput::Continue,
            AgentInput::Shutdown,
            AgentInput::Compact,
            AgentInput::Clear,
            AgentInput::PermissionResponse {
                req_id: "r1".to_string(),
                approved: true,
                remember: false,
            },
            AgentInput::AskUserResponse {
                req_id: "r2".to_string(),
                response: crate::tools::AskUserResponse {
                    answers: Default::default(),
                },
            },
        ] {
            bus.publish(SessionId::from("s1"), input).unwrap();
        }
    });

    // No cancel token in ctx: non-steer inputs must not wake the sleep.
    let output = tool.exec(json!({"seconds": 5}), ctx("s1")).await.unwrap();

    assert_eq!(text_of(&output), "Slept for 5 seconds");
}

#[tokio::test(start_paused = true)]
async fn test_cancel_token_still_wakes() {
    let tool = SleepTool::new(None);
    let token = tokio_util::sync::CancellationToken::new();

    tokio::spawn({
        let token = token.clone();
        async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            token.cancel();
        }
    });

    let output = tool
        .exec(
            json!({"seconds": 60}),
            ctx("s1").with_cancel_token(Some(token)),
        )
        .await
        .unwrap();

    assert_eq!(
        text_of(&output),
        "Sleep cancelled after 5 seconds (planned 60 seconds, not completed)"
    );
}

#[test]
fn test_should_wake_filter() {
    assert!(should_wake(&AgentInput::Steer(vec![])));
    assert!(!should_wake(&user_input()));
    assert!(!should_wake(&AgentInput::Continue));
    assert!(!should_wake(&AgentInput::Shutdown));
    assert!(!should_wake(&AgentInput::Compact));
    assert!(!should_wake(&AgentInput::Clear));
    assert!(!should_wake(&AgentInput::Cancel));
    assert!(!should_wake(&AgentInput::PermissionResponse {
        req_id: "r".to_string(),
        approved: true,
        remember: false,
    }));
    assert!(!should_wake(&AgentInput::AskUserResponse {
        req_id: "r".to_string(),
        response: crate::tools::AskUserResponse {
            answers: Default::default(),
        },
    }));
}
