use super::*;

#[test]
fn delta_below_threshold_yields_no_summary() {
    let mut state = StreamCollectorState::default();
    // 16000 bytes -> 4000 estimated tokens (< 4096).
    let delta = "a".repeat(16000);
    assert!(state
        .handle_tool_call_delta("call_1", "write", &delta)
        .is_none());
}

#[test]
fn summary_emitted_at_each_4k_token_boundary() {
    let mut state = StreamCollectorState::default();

    // 16000 bytes -> 4000 tokens: below first threshold.
    let delta = "a".repeat(16000);
    assert!(state
        .handle_tool_call_delta("call_1", "write", &delta)
        .is_none());

    // 16500 bytes -> 4125 tokens: crosses 4096.
    let delta = "a".repeat(500);
    let summary = state
        .handle_tool_call_delta("call_1", "write", &delta)
        .expect("summary at 4k boundary");
    assert!(summary.contains("`write`"));
    assert!(summary.contains("call_1"));
    assert!(summary.contains("~4.1k"));

    // 32500 bytes -> 8125 tokens: below next threshold (8192).
    let delta = "a".repeat(16000);
    assert!(state
        .handle_tool_call_delta("call_1", "write", &delta)
        .is_none());

    // 32800 bytes -> 8200 tokens: crosses 8192.
    let delta = "a".repeat(300);
    let summary = state
        .handle_tool_call_delta("call_1", "write", &delta)
        .expect("summary at 8k boundary");
    assert!(summary.contains("~8.2k"));
}

#[test]
fn huge_single_delta_logs_once_and_skips_passed_boundaries() {
    let mut state = StreamCollectorState::default();
    // 40000 bytes -> 10000 tokens in one shot: logs once, next threshold 12288.
    let delta = "a".repeat(40000);
    let summary = state
        .handle_tool_call_delta("call_1", "write", &delta)
        .expect("summary for huge delta");
    assert!(summary.contains("~10.0k"));

    // 40900 bytes -> 10225 tokens: still below 12288.
    let delta = "a".repeat(900);
    assert!(state
        .handle_tool_call_delta("call_1", "write", &delta)
        .is_none());

    // 49200 bytes -> 12300 tokens: crosses 12288.
    let delta = "a".repeat(8300);
    assert!(state
        .handle_tool_call_delta("call_1", "write", &delta)
        .is_some());
}

#[test]
fn new_tool_call_id_resets_tracker() {
    let mut state = StreamCollectorState::default();
    // call_1 accumulates 16000 bytes (4000 tokens), just below the threshold.
    let delta = "a".repeat(16000);
    assert!(state
        .handle_tool_call_delta("call_1", "write", &delta)
        .is_none());
    // call_2 starts fresh: its own small delta stays far below the threshold.
    let delta = "a".repeat(500);
    assert!(state
        .handle_tool_call_delta("call_2", "bash", &delta)
        .is_none());
    // A late delta for call_1 no longer accumulates onto its old state.
    assert!(state
        .handle_tool_call_delta("call_1", "write", &delta)
        .is_none());
    // call_2's contiguous stream crosses the boundary on its own.
    let delta = "a".repeat(16400);
    assert!(state
        .handle_tool_call_delta("call_2", "bash", &delta)
        .is_some());
}

#[test]
fn summary_head_snippet_truncated_at_char_boundary() {
    let mut state = StreamCollectorState::default();
    let first = format!("{{\"path\":\"{}\",", "好".repeat(100));
    assert!(state
        .handle_tool_call_delta("call_1", "write", &first)
        .is_none());

    let delta = "a".repeat(17000);
    let summary = state
        .handle_tool_call_delta("call_1", "write", &delta)
        .expect("summary after crossing threshold");
    let expected_head: String = first.chars().take(80).collect();
    assert!(summary.contains(&format!("{expected_head:?}")));
}

#[test]
fn summary_tail_snippet_keeps_last_chars() {
    let mut state = StreamCollectorState::default();
    let first = r#"{"path":"x","content":""#;
    assert!(state
        .handle_tool_call_delta("call_1", "write", first)
        .is_none());

    let delta = format!("{}{}", "b".repeat(16900), "尾".repeat(80));
    let summary = state
        .handle_tool_call_delta("call_1", "write", &delta)
        .expect("summary after crossing threshold");

    // Tail is the last 80 chars of the stream.
    let expected_tail = "尾".repeat(80);
    assert!(summary.contains(&format!("tail: {expected_tail:?}")));

    // Head grew across deltas up to the 80-char cap.
    let expected_head: String = first.chars().chain("b".chars().cycle().take(57)).collect();
    assert_eq!(expected_head.chars().count(), 80);
    assert!(summary.contains(&format!("head: {expected_head:?}")));
}

#[test]
fn empty_tool_name_filled_by_later_deltas() {
    let mut state = StreamCollectorState::default();
    // Providers may emit args deltas before the tool name chunk arrives.
    assert!(state.handle_tool_call_delta("call_1", "", "a").is_none());
    let delta = "a".repeat(16400);
    let summary = state
        .handle_tool_call_delta("call_1", "write", &delta)
        .expect("summary after crossing threshold");
    assert!(summary.contains("`write`"));
}

#[test]
fn empty_first_delta_head_filled_by_later_deltas() {
    let mut state = StreamCollectorState::default();
    // Providers may emit an initial empty delta to signal the call start.
    assert!(state.handle_tool_call_delta("call_1", "bash", "").is_none());
    assert!(state
        .handle_tool_call_delta("call_1", "bash", r#"{"cmd":"ls"}"#)
        .is_none());

    let delta = "a".repeat(16400);
    let summary = state
        .handle_tool_call_delta("call_1", "bash", &delta)
        .expect("summary after crossing threshold");
    assert!(summary.contains(r#"head: "{\"cmd\":\"ls\"}"#));
}

#[test]
fn metadata_alone_is_not_persistable_payload() {
    // The 03:57 incident shape: model hiccups an empty completion — no
    // content, no tool calls, only usage + response id + a weird finish
    // reason. This must never become a persisted assistant message.
    let result = StreamCollectionResult {
        token_usage: Some(crate::provider::TokenUsage::new(500, 1, None)),
        response_id: Some("chatcmpl-poison".into()),
        finish_reason: Some(FinishReason::Unknown),
        ..StreamCollectionResult::default()
    };
    assert!(!result.has_persistable_payload());
}

#[test]
fn content_or_tool_calls_are_persistable_payload() {
    let text_only = StreamCollectionResult {
        content_blocks: vec![ContentBlock::Text { text: "hi".into() }],
        ..StreamCollectionResult::default()
    };
    assert!(text_only.has_persistable_payload());

    let calls_only = StreamCollectionResult {
        tool_calls: vec![ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        }],
        ..StreamCollectionResult::default()
    };
    assert!(calls_only.has_persistable_payload());
}
