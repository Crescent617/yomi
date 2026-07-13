use super::*;

#[test]
fn test_parse_kimi_usage_with_cached_tokens() {
    // Real SSE data from Kimi API - usage is in choices[0].usage
    // Note: Kimi puts usage INSIDE choices, not at top level
    let data = r#"{"id":"chatcmpl-69f75d4e42d433402b5cfc09","object":"chat.completion.chunk","created":1777818959,"model":"kimi-k2.5","choices":[{"index":0,"delta":{},"finish_reason":"stop","usage":{"prompt_tokens":8,"completion_tokens":113,"total_tokens":121,"prompt_tokens_details":{"cached_tokens":8}}}]}"#;

    let response: OpenAIStreamResponse = serde_json::from_str(data).unwrap();

    let choice = response
        .choices
        .into_iter()
        .next()
        .expect("should have one choice");
    assert!(choice.usage.is_some(), "choice.usage should be present");
    let usage = choice.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 8);
    assert_eq!(usage.completion_tokens, 113);
    assert_eq!(usage.cached_tokens(), Some(8), "cached_tokens should be 8");
}

#[test]
fn test_parse_openai_usage_with_cached_tokens() {
    // OpenAI format: cached_tokens in prompt_tokens_details (nested)
    // Note: OpenAI puts usage INSIDE choices
    let data = r#"{"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1777819000,"model":"gpt-4","choices":[{"index":0,"delta":{},"finish_reason":"stop","usage":{"prompt_tokens":19,"completion_tokens":10,"total_tokens":29,"prompt_tokens_details":{"cached_tokens":5}}}]}"#;

    let response: OpenAIStreamResponse = serde_json::from_str(data).unwrap();

    let choice = response
        .choices
        .into_iter()
        .next()
        .expect("should have one choice");
    assert!(choice.usage.is_some(), "choice.usage should be present");
    let usage = choice.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 19);
    assert_eq!(usage.completion_tokens, 10);
    assert_eq!(usage.cached_tokens(), Some(5), "cached_tokens should be 5");
}

#[test]
fn test_parse_top_level_usage_with_cached_tokens() {
    // Some providers put usage at TOP LEVEL instead of inside choices
    let data = r#"{"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1777819000,"model":"test","usage":{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150,"prompt_tokens_details":{"cached_tokens":25}},"choices":[]}"#;

    let response: OpenAIStreamResponse = serde_json::from_str(data).unwrap();

    // Top-level usage should be detected
    assert!(
        response.usage.is_some(),
        "top-level usage should be present"
    );
    let usage = response.usage.unwrap();
    assert_eq!(usage.prompt_tokens, 100);
    assert_eq!(usage.completion_tokens, 50);
    assert_eq!(
        usage.cached_tokens(),
        Some(25),
        "cached_tokens should be 25"
    );
}

fn create_test_response(delta: OpenAIDelta) -> OpenAIStreamResponse {
    OpenAIStreamResponse {
        id: None,
        choices: vec![OpenAIChoice {
            delta: Some(delta),
            usage: None,
            finish_reason: None,
        }],
        usage: None,
    }
}

fn create_tool_call_delta(
    index: usize,
    id: Option<&str>,
    name: Option<&str>,
    args: Option<&str>,
) -> OpenAIDelta {
    OpenAIDelta {
        content: None,
        thinking: None,
        reasoning: None,
        reasoning_content: None,
        thinking_signature: None,
        thinking_redacted: None,
        tool_calls: Some(vec![OpenAIToolCall {
            index: Some(index),
            id: id.map(|s| s.to_string()),
            type_: Some("function".to_string()),
            function: OpenAIFunction {
                name: name.map(|s| s.to_string()),
                arguments: args.map(|s| s.to_string()),
            },
        }]),
    }
}

#[test]
fn test_assembler_single_tool_call() {
    let mut assembler = MsgChunkAssembler::new();

    // First chunk: tool call starts
    let delta = create_tool_call_delta(0, Some("call_123"), Some("bash"), Some("{\"cmd\":\""));
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    // ToolCallDelta is emitted for UI feedback since id is available
    assert_eq!(items.len(), 1);
    assert!(matches!(&items[0], ModelStreamItem::ToolCallDelta { id, .. } if id == "call_123"));

    // Second chunk: arguments continue
    let delta = create_tool_call_delta(0, None, None, Some("ls"));
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    // ToolCallDelta is emitted for the argument delta
    assert_eq!(items.len(), 1);
    assert!(
        matches!(&items[0], ModelStreamItem::ToolCallDelta { arguments_delta, .. } if arguments_delta == "ls")
    );

    // Third chunk: arguments complete
    let delta = create_tool_call_delta(0, None, None, Some("\"}"));
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    // ToolCallDelta is emitted for the argument delta
    assert_eq!(items.len(), 1);
    assert!(
        matches!(&items[0], ModelStreamItem::ToolCallDelta { arguments_delta, .. } if arguments_delta == "\"}")
    );

    // Finish should emit the completed tool call
    let items = assembler.finish();
    assert_eq!(items.len(), 2); // ToolCall + Complete

    match &items[0] {
        ModelStreamItem::ToolCall(call) => {
            assert_eq!(call.id, "call_123");
            assert_eq!(call.name, "bash");
            assert_eq!(call.arguments, serde_json::json!({"cmd":"ls"}));
        }
        _ => panic!("Expected ToolCall, got {:?}", items[0]),
    }
    assert!(matches!(items[1], ModelStreamItem::Complete));
}

#[test]
fn test_assembler_multiple_tool_calls() {
    let mut assembler = MsgChunkAssembler::new();

    // First tool call starts
    let delta = create_tool_call_delta(
        0,
        Some("call_1"),
        Some("read"),
        Some("{\"path\":\"file.txt\"}"),
    );
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);
    // ToolCallDelta is emitted for UI feedback
    assert_eq!(items.len(), 1);
    assert!(matches!(&items[0], ModelStreamItem::ToolCallDelta { id, .. } if id == "call_1"));

    // Second tool call starts - this should complete the first one
    let delta = create_tool_call_delta(
        1,
        Some("call_2"),
        Some("write"),
        Some("{\"path\":\"out.txt\"}"),
    );
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    // Should emit first tool call immediately when second starts (ToolCall)
    // and ToolCallDelta for the second call
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], ModelStreamItem::ToolCall(call) if call.id == "call_1"));
    assert!(matches!(&items[1], ModelStreamItem::ToolCallDelta { id, .. } if id == "call_2"));

    // Finish should emit second tool call
    let items = assembler.finish();
    assert_eq!(items.len(), 2); // ToolCall + Complete
    match &items[0] {
        ModelStreamItem::ToolCall(call) => {
            assert_eq!(call.id, "call_2");
            assert_eq!(call.name, "write");
        }
        _ => panic!("Expected ToolCall"),
    }
}

#[test]
fn test_assembler_text_content() {
    let mut assembler = MsgChunkAssembler::new();

    let delta = OpenAIDelta {
        content: Some("Hello".to_string()),
        thinking: None,
        reasoning: None,
        reasoning_content: None,
        thinking_signature: None,
        thinking_redacted: None,
        tool_calls: None,
    };
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::Chunk(ContentChunk::Text(text)) => {
            assert_eq!(text, "Hello");
        }
        _ => panic!("Expected Text chunk"),
    }
}

#[test]
fn test_assembler_thinking_content() {
    let mut assembler = MsgChunkAssembler::new();

    let delta = OpenAIDelta {
        content: None,
        thinking: Some("Let me think...".to_string()),
        reasoning: None,
        reasoning_content: None,
        thinking_signature: Some("sig123".to_string()),
        thinking_redacted: None,
        tool_calls: None,
    };
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::Chunk(ContentChunk::Thinking {
            thinking,
            signature,
        }) => {
            assert_eq!(thinking, "Let me think...");
            assert_eq!(signature.as_deref(), Some("sig123"));
        }
        _ => panic!("Expected Thinking chunk, got {:?}", items[0]),
    }
}

#[test]
fn test_assembler_reasoning_content_fallback() {
    let mut assembler = MsgChunkAssembler::new();

    // Test reasoning field (used by some providers)
    let delta = OpenAIDelta {
        content: None,
        thinking: None,
        reasoning: Some("Reasoning step".to_string()),
        reasoning_content: None,
        thinking_signature: None,
        thinking_redacted: None,
        tool_calls: None,
    };
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::Chunk(ContentChunk::Thinking { thinking, .. }) => {
            assert_eq!(thinking, "Reasoning step");
        }
        _ => panic!("Expected Thinking chunk"),
    }
}

#[test]
fn test_assembler_redacted_thinking() {
    let mut assembler = MsgChunkAssembler::new();

    let delta = OpenAIDelta {
        content: None,
        thinking: None,
        reasoning: None,
        reasoning_content: None,
        thinking_signature: None,
        thinking_redacted: Some(true),
        tool_calls: None,
    };
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    assert_eq!(items.len(), 1);
    assert!(matches!(
        items[0],
        ModelStreamItem::Chunk(ContentChunk::RedactedThinking)
    ));
}

#[test]
fn test_assembler_empty_content_filtered() {
    let mut assembler = MsgChunkAssembler::new();

    // Empty content should be filtered out
    let delta = OpenAIDelta {
        content: Some(String::new()),
        thinking: None,
        reasoning: None,
        reasoning_content: None,
        thinking_signature: None,
        thinking_redacted: None,
        tool_calls: None,
    };
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let items = assembler.process(&json);

    assert!(items.is_empty());
}

#[test]
fn test_assembler_no_choices() {
    let mut assembler = MsgChunkAssembler::new();

    let response = OpenAIStreamResponse {
        id: None,
        choices: vec![],
        usage: None,
    };
    let json = serde_json::to_string(&response).unwrap();
    let items = assembler.process(&json);

    assert!(items.is_empty());
}

#[test]
fn test_assembler_no_delta() {
    let mut assembler = MsgChunkAssembler::new();

    let response = OpenAIStreamResponse {
        id: None,
        choices: vec![OpenAIChoice {
            delta: None,
            usage: None,
            finish_reason: None,
        }],
        usage: None,
    };
    let json = serde_json::to_string(&response).unwrap();
    let items = assembler.process(&json);

    assert!(items.is_empty());
}

#[test]
fn test_assembler_invalid_json_ignored() {
    let mut assembler = MsgChunkAssembler::new();

    let items = assembler.process("invalid json");
    assert!(
        items.is_empty(),
        "invalid JSON should be ignored with warning"
    );
}

#[test]
fn test_assembler_incomplete_tool_call_finish() {
    let mut assembler = MsgChunkAssembler::new();

    // Start a tool call but never complete it
    let delta = create_tool_call_delta(0, Some("call_1"), None, None); // missing name
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    let _ = assembler.process(&json);

    // Finish should not emit incomplete tool call (no name)
    let items = assembler.finish();
    assert_eq!(items.len(), 1); // Just Complete, no ToolCall
    assert!(matches!(items[0], ModelStreamItem::Complete));
}

#[test]
fn test_assembler_finish_reason_and_usage_in_same_chunk() {
    let mut assembler = MsgChunkAssembler::new();

    // Real data from Kimi/free-tokens proxy: last chunk has both finish_reason and top-level usage
    let data = r#"{"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567890,"model":"kimi-k2.7-code-highspeed","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":16,"total_tokens":27}}"#;

    let items = assembler.process(data);
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ModelStreamItem::TokenUsage(crate::provider::TokenUsage {
            prompt_tokens: 11,
            completion_tokens: 16,
            ..
        })
    ));

    // finish() should emit ResponseMeta with captured finish_reason
    let items = assembler.finish();
    assert_eq!(items.len(), 2); // ResponseMeta + Complete
    assert!(matches!(
        &items[0],
        ModelStreamItem::ResponseMeta {
            response_id,
            finish_reason: Some(FinishReason::Stop),
        } if response_id.as_deref() == Some("chatcmpl-test")
    ));
    assert!(matches!(items[1], ModelStreamItem::Complete));
}

#[test]
fn test_assembler_choice_usage_only() {
    let mut assembler = MsgChunkAssembler::new();

    // Some providers (like older Kimi) put usage only inside the choice
    let data = r#"{"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567890,"model":"kimi-k2.5","choices":[{"index":0,"delta":{},"finish_reason":"stop","usage":{"prompt_tokens":8,"completion_tokens":113,"total_tokens":121}}]}"#;

    let items = assembler.process(data);
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ModelStreamItem::TokenUsage(crate::provider::TokenUsage {
            prompt_tokens: 8,
            completion_tokens: 113,
            ..
        })
    ));

    let items = assembler.finish();
    assert_eq!(items.len(), 2);
    assert!(matches!(
        &items[0],
        ModelStreamItem::ResponseMeta {
            response_id,
            finish_reason: Some(FinishReason::Stop),
        } if response_id.as_deref() == Some("chatcmpl-test")
    ));
    assert!(matches!(items[1], ModelStreamItem::Complete));
}

#[test]
fn test_assembler_empty_choices_with_usage() {
    let mut assembler = MsgChunkAssembler::new();

    // Some proxies send a chunk with empty choices but usage after the finish_reason chunk
    let data = r#"{"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567890,"model":"kimi-k2.7-code-highspeed","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":16,"total_tokens":27}}"#;

    let items = assembler.process(data);
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ModelStreamItem::TokenUsage(crate::provider::TokenUsage {
            prompt_tokens: 11,
            completion_tokens: 16,
            ..
        })
    ));
}

#[test]
fn test_assembler_usage_without_delta() {
    let mut assembler = MsgChunkAssembler::new();

    // Chunk with finish_reason but no delta (empty delta object)
    let data = r#"{"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{},"finish_reason":"length","usage":null}],"usage":{"prompt_tokens":100,"completion_tokens":50,"total_tokens":150}}"#;

    let items = assembler.process(data);
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ModelStreamItem::TokenUsage(crate::provider::TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            ..
        })
    ));

    let items = assembler.finish();
    assert_eq!(items.len(), 2); // ResponseMeta + Complete
    assert!(matches!(
        &items[0],
        ModelStreamItem::ResponseMeta {
            response_id,
            finish_reason: Some(FinishReason::MaxTokens),
        } if response_id.as_deref() == Some("chatcmpl-test")
    ));
}
