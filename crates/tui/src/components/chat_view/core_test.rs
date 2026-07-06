use super::*;

#[allow(dead_code)]
fn make_tool_msg(tool_id: &str, tool_name: &str) -> HistoryMessage {
    HistoryMessage::Tool {
        tool_name: tool_name.to_string(),
        tool_id: tool_id.to_string(),
        status: ToolStatus::Running,
        output: None,
        error: None,
        folded: true,
        arguments: None,
        parsed_args: None,
        elapsed_ms: None,
        content_blocks: Vec::new(),
        subagent: None,
    }
}

#[test]
fn test_init_subagent() {
    let mut cv = ChatView::new();
    cv.start_tool("call_01".to_string(), "agent".to_string(), None);

    cv.init_subagent("call_01", "sub_abc".to_string(), "Audit deps".to_string());

    let msg = &cv.messages[0];
    assert!(
        matches!(msg, HistoryMessage::Tool { tool_id, subagent: Some(sa), .. } if tool_id == "call_01" && sa.session_id == "sub_abc" && sa.description == "Audit deps")
    );
}

#[test]
fn test_update_subagent_tool_event() {
    let mut cv = ChatView::new();
    cv.start_tool("call_01".to_string(), "agent".to_string(), None);
    cv.init_subagent("call_01", "sub_abc".to_string(), "Audit deps".to_string());

    let event = kernel::event::Event::Tool(kernel::event::ToolEvent::Start {
        tool_name: "read".to_string(),
        tool_id: "tc_1".to_string(),
        message_id: kernel::types::MessageId::new(),
        arguments: Some("{}".to_string()),
    });
    cv.update_subagent("call_01", event);

    if let HistoryMessage::Tool {
        subagent: Some(ref sa),
        ..
    } = cv.messages[0]
    {
        assert_eq!(sa.events.len(), 1);
        assert!(
            matches!(&sa.events[0], kernel::event::Event::Tool(kernel::event::ToolEvent::Start { tool_name, .. }) if tool_name == "read")
        );
    } else {
        panic!("Expected subagent");
    }
}

#[test]
fn test_update_subagent_stopped() {
    let mut cv = ChatView::new();
    cv.start_tool("call_01".to_string(), "agent".to_string(), None);
    cv.init_subagent("call_01", "sub_abc".to_string(), "Audit deps".to_string());

    let event = kernel::event::Event::Agent(kernel::event::AgentEvent::Lifecycle {
        state: kernel::event::AgentStatus::Stopped {
            reason: kernel::event::StopReason::Completed {
                finish_reason: Some(kernel::types::FinishReason::Stop),
            },
        },
    });
    cv.update_subagent("call_01", event);

    if let HistoryMessage::Tool {
        subagent: Some(ref sa),
        ..
    } = cv.messages[0]
    {
        assert!(matches!(sa.status, SubagentStatus::Completed));
        assert_eq!(sa.events.len(), 1);
    } else {
        panic!("Expected subagent");
    }
}

#[test]
fn test_update_subagent_token_usage() {
    let mut cv = ChatView::new();
    cv.start_tool("call_01".to_string(), "agent".to_string(), None);
    cv.init_subagent("call_01", "sub_abc".to_string(), "Audit deps".to_string());

    let event = kernel::event::Event::Model(kernel::event::ModelEvent::TokenUsage {
        prompt_tokens: 42,
        completion_tokens: 10,
        total_tokens: 52,
        message_id: kernel::types::MessageId::new(),
        context_window: 4096,
    });
    cv.update_subagent("call_01", event);

    if let HistoryMessage::Tool {
        subagent: Some(ref sa),
        ..
    } = cv.messages[0]
    {
        assert_eq!(sa.total_prompt_tokens, 42);
        assert_eq!(sa.total_completion_tokens, 10);
    } else {
        panic!("Expected subagent");
    }
}

#[test]
fn test_finalize_subagent() {
    let mut cv = ChatView::new();
    cv.start_tool("call_01".to_string(), "agent".to_string(), None);
    cv.init_subagent("call_01", "sub_abc".to_string(), "Audit deps".to_string());

    cv.finalize_subagent("call_01");

    if let HistoryMessage::Tool {
        subagent: Some(ref sa),
        ..
    } = cv.messages[0]
    {
        assert!(matches!(sa.status, SubagentStatus::Completed));
    } else {
        panic!("Expected subagent");
    }
}

#[test]
fn test_update_subagent_no_match() {
    let mut cv = ChatView::new();
    cv.start_tool("call_01".to_string(), "agent".to_string(), None);
    cv.init_subagent("call_01", "sub_abc".to_string(), "Audit deps".to_string());

    let event = kernel::event::Event::Tool(kernel::event::ToolEvent::Start {
        tool_name: "read".to_string(),
        tool_id: "tc_1".to_string(),
        message_id: kernel::types::MessageId::new(),
        arguments: Some("{}".to_string()),
    });
    cv.update_subagent("nonexistent", event);

    // Subagent should remain unchanged
    if let HistoryMessage::Tool {
        subagent: Some(ref sa),
        ..
    } = cv.messages[0]
    {
        assert!(sa.events.is_empty());
    } else {
        panic!("Expected subagent");
    }
}
