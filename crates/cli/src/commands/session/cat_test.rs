use super::*;
use kernel::types::{Message, MessageId, ToolCall};
use std::collections::HashMap;
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    PathBuf::from("/tmp/yomi-test-data")
}

/// Assistant message carrying a tool call, paired with its tool result.
fn tool_pair(name: &str, args: serde_json::Value, result: &str) -> Vec<Message> {
    let mut assistant = Message::assistant("");
    assistant.content = vec![];
    assistant.tool_calls = Some(vec![ToolCall {
        id: "call-1".to_string(),
        name: name.to_string(),
        arguments: args,
    }]);
    vec![
        assistant,
        Message::tool_result(MessageId::new(), "call-1", result),
    ]
}

#[test]
fn user_and_assistant_text_only() {
    let messages = vec![
        Message::system("system prompt"),
        Message::user("hello"),
        Message::assistant("hi there"),
    ];
    let out = format_transcript(messages, &data_dir(), false);
    assert!(out.contains("=== user · "));
    assert!(out.contains("hello"));
    assert!(out.contains("=== assistant · "));
    assert!(out.contains("hi there"));
    assert!(!out.contains("system prompt"));
}

#[test]
fn thinking_is_skipped() {
    let messages = vec![Message::assistant_with_thinking(
        "answer",
        "secret reasoning",
    )];
    let out = format_transcript(messages, &data_dir(), false);
    assert!(out.contains("answer"));
    assert!(!out.contains("secret reasoning"));
}

#[test]
fn image_asset_resolves_to_real_path() {
    let messages = vec![Message::user_with_image(
        "look at this",
        "asset://abc123.png",
    )];
    let out = format_transcript(messages, &data_dir(), false);
    assert!(out.contains("[image: /tmp/yomi-test-data/assets/abc123.png]"));
    assert!(!out.contains("asset://"));
}

#[test]
fn inline_base64_image_is_summarized() {
    let messages = vec![Message::user_with_image(
        "pic",
        "data:image/png;base64,iVBORw0KGgo=",
    )];
    let out = format_transcript(messages, &data_dir(), false);
    assert!(out.contains("[image: (inline base64 image)]"));
}

#[test]
fn empty_turns_are_skipped() {
    // Assistant message with no text content and no tool calls.
    let mut empty = Message::assistant("");
    empty.content = vec![];
    let messages = vec![empty, Message::user("next")];
    let out = format_transcript(messages, &data_dir(), false);
    assert!(!out.contains("assistant"));
    assert!(out.contains("next"));
}

#[test]
fn no_displayable_messages_returns_empty() {
    let messages = vec![Message::system("sys")];
    assert!(format_transcript(messages, &data_dir(), false).is_empty());
}

#[test]
fn steer_message_is_labeled() {
    let mut msg = Message::user("steer note");
    msg.metadata = Some(HashMap::from([(
        "is_steer".to_string(),
        "true".to_string(),
    )]));
    let out = format_transcript(vec![msg], &data_dir(), false);
    assert!(out.contains("=== user (steer) · "));
}

#[test]
fn tool_message_shows_name_args_and_result() {
    let messages = tool_pair(
        "read",
        serde_json::json!({"path": "/tmp/x.rs"}),
        "file contents",
    );
    let out = format_transcript(messages, &data_dir(), true);
    assert!(out.contains("=== tool · read · "));
    assert!(out.contains(r#"args: {"path":"/tmp/x.rs"}"#));
    assert!(out.contains("file contents"));
    // tool-call-only assistant turn itself stays hidden
    assert!(!out.contains("=== assistant · "));
}

#[test]
fn tool_result_image_resolves_to_real_path() {
    let mut tool_msg = Message::tool_result(MessageId::new(), "call-1", "shot taken");
    tool_msg.content.push(ContentBlock::ImageUrl {
        image_url: kernel::types::ImageUrl {
            url: "asset://deadbeef.png".to_string(),
            detail: None,
        },
    });
    let mut assistant = Message::assistant("");
    assistant.content = vec![];
    assistant.tool_calls = Some(vec![ToolCall {
        id: "call-1".to_string(),
        name: "browser".to_string(),
        arguments: serde_json::json!({}),
    }]);
    let out = format_transcript(vec![assistant, tool_msg], &data_dir(), true);
    assert!(out.contains("[image: /tmp/yomi-test-data/assets/deadbeef.png]"));
}

#[test]
fn orphan_tool_result_is_skipped() {
    // No matching assistant tool_call → dropped by from_storage
    let messages = vec![Message::tool_result(
        MessageId::new(),
        "missing-call",
        "orphan output",
    )];
    let out = format_transcript(messages, &data_dir(), true);
    assert!(!out.contains("orphan output"));
}

#[test]
fn long_tool_args_are_truncated() {
    let big = "x".repeat(1000);
    let messages = tool_pair("write", serde_json::json!({"content": big}), "ok");
    let out = format_transcript(messages, &data_dir(), true);
    assert!(out.contains("args: "));
    assert!(out.contains("..."));
    // args capped well below the raw 1000-char payload
    assert!(!out.contains(&big));
}

#[test]
fn long_tool_result_is_truncated() {
    let big = "y".repeat(5000);
    let messages = tool_pair("shell", serde_json::json!({"command": "ls"}), &big);
    let out = format_transcript(messages, &data_dir(), true);
    assert!(out.contains("...[truncated]"));
    assert!(!out.contains(&big));
}

#[test]
fn dangling_tool_call_is_shown_as_interrupted() {
    // Assistant issued a tool call but no result message exists (cancel/crash).
    let mut assistant = Message::assistant("let me check");
    assistant.tool_calls = Some(vec![ToolCall {
        id: "call-9".to_string(),
        name: "shell".to_string(),
        arguments: serde_json::json!({"command": "make"}),
    }]);
    let out = format_transcript(vec![assistant], &data_dir(), true);
    assert!(out.contains("=== assistant · "));
    assert!(out.contains("let me check"));
    assert!(out.contains("=== tool · shell · "));
    assert!(out.contains(r#"args: {"command":"make"}"#));
    assert!(out.contains("(no result — interrupted?)"));
}

#[test]
fn tools_hidden_without_flag() {
    // Paired tool call + dangling call: both invisible without show_tools.
    let mut messages = tool_pair("read", serde_json::json!({"path": "/tmp/x.rs"}), "contents");
    let mut dangling_assistant = Message::assistant("trying");
    dangling_assistant.tool_calls = Some(vec![ToolCall {
        id: "call-9".to_string(),
        name: "shell".to_string(),
        arguments: serde_json::json!({"command": "make"}),
    }]);
    messages.push(dangling_assistant);
    let out = format_transcript(messages, &data_dir(), false);
    assert!(!out.contains("=== tool · "));
    assert!(!out.contains("args: "));
    assert!(!out.contains("no result"));
    // user/assistant text still shown
    assert!(out.contains("trying"));
}

#[test]
fn redact_base64_elides_large_payload() {
    let blob = "A".repeat(5000);
    let line = format!(r#"{{"url":"data:image/png;base64,{blob}"}}"#);
    let out = redact_base64(&line);
    assert_eq!(out, r#"{"url":"data:image/png;base64,[omitted:5000]"}"#);
    // still valid JSON
    assert!(serde_json::from_str::<serde_json::Value>(&out).is_ok());
}

#[test]
fn redact_base64_keeps_small_payload() {
    let line = r#"{"url":"data:image/gif;base64,R0lGODlhAQABAAAAAC"}"#;
    assert_eq!(redact_base64(line), line);
}

#[test]
fn redact_base64_handles_multiple_and_trailing() {
    let blob = "B".repeat(1000);
    let line = format!(r#"["data:image/png;base64,{blob}","data:image/gif;base64,{blob}"]"#);
    let out = redact_base64(&line);
    assert_eq!(
        out,
        r#"["data:image/png;base64,[omitted:1000]","data:image/gif;base64,[omitted:1000]"]"#
    );
}

#[test]
fn redact_base64_ignores_plain_text() {
    // ";base64," outside a data URL must stay verbatim even when long.
    let blob = "C".repeat(1000);
    let line = format!(r#"{{"role":"user","content":"see ;base64,{blob} here"}}"#);
    assert_eq!(redact_base64(&line), line);
}

#[test]
fn redact_base64_no_marker_is_identity() {
    let line = r#"{"role":"user","content":"hello"}"#;
    assert_eq!(redact_base64(line), line);
}
