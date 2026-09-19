use super::*;
use crate::types::{ContentBlock, ImageUrl, ToolCall};

#[test]
fn test_parse_choice_level_usage_with_cached_tokens() {
    // Real SSE data from an OpenAI-compatible API - usage is in choices[0].usage
    // Note: usage is INSIDE choices, not at top level
    let data = r#"{"id":"chatcmpl-69f75d4e42d433402b5cfc09","object":"chat.completion.chunk","created":1777818959,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop","usage":{"prompt_tokens":8,"completion_tokens":113,"total_tokens":121,"prompt_tokens_details":{"cached_tokens":8}}}]}"#;

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
fn test_assembler_tool_completion_reports_tool_calls_finish_reason() {
    let mut assembler = MsgChunkAssembler::new();

    let delta = create_tool_call_delta(0, Some("call_123"), Some("bash"), Some(r#"{"cmd":"ls"}"#));
    let json = serde_json::to_string(&create_test_response(delta)).unwrap();
    assembler.process(&json);

    let terminal = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#;
    assembler.process(terminal);
    let items = assembler.finish();

    assert!(items
        .iter()
        .any(|item| matches!(item, ModelStreamItem::ToolCall(_))));
    assert!(items.iter().any(|item| matches!(
        item,
        ModelStreamItem::ResponseMeta {
            response_id: None,
            finish_reason: Some(FinishReason::ToolCalls),
        }
    )));
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
fn test_assembler_premature_end_is_retryable_and_does_not_flush_partial_call() {
    let mut assembler = MsgChunkAssembler::new();
    let delta = create_tool_call_delta(0, Some("call_123"), Some("bash"), Some("{\"cmd\":"));
    assembler.process(&serde_json::to_string(&create_test_response(delta)).unwrap());

    let err = assembler.finish_stream("EOF").unwrap_err();
    assert!(matches!(err, ProviderError::Sse(_)));
    assert!(err.is_retryable());
    assert!(!assembler.finished);
    assert_eq!(assembler.partials.len(), 1);
}

#[test]
fn test_assembler_done_sentinel_requires_finish_reason() {
    let mut assembler = MsgChunkAssembler::new();
    let err = assembler.finish_stream("[DONE]").unwrap_err();
    assert!(matches!(err, ProviderError::Sse(_)));
    assert!(err.is_retryable());
    assert!(!assembler.finished);
}
#[test]
fn test_assembler_done_sentinel_after_finish_reason_completes() {
    let mut assembler = MsgChunkAssembler::new();
    assembler.process(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#);

    let items = assembler.finish_stream("[DONE]").unwrap();
    assert!(assembler.finished);
    assert!(items.iter().any(|item| matches!(
        item,
        ModelStreamItem::ResponseMeta {
            finish_reason: Some(FinishReason::Stop),
            ..
        }
    )));
    assert!(items
        .iter()
        .any(|item| matches!(item, ModelStreamItem::Complete)));
}

#[test]
fn test_assembler_empty_content_filtered() {
    let mut assembler = MsgChunkAssembler::new();
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

    // Real data from an OpenAI-compatible free-tokens proxy: last chunk has both finish_reason and top-level usage
    let data = r#"{"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567890,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":16,"total_tokens":27}}"#;

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

    // Some providers put usage only inside the choice
    let data = r#"{"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567890,"model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop","usage":{"prompt_tokens":8,"completion_tokens":113,"total_tokens":121}}]}"#;

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
    let data = r#"{"id":"chatcmpl-test","object":"chat.completion.chunk","created":1234567890,"model":"test-model","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":16,"total_tokens":27}}"#;

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

// ==== convert_messages: tool-result image handling ====

fn assistant_with_tool_call(call_id: &str) -> Arc<Message> {
    Arc::new(Message {
        role: Role::Assistant,
        content: vec![],
        tool_calls: Some(vec![ToolCall {
            id: call_id.into(),
            name: "screenshot".into(),
            arguments: serde_json::json!({}),
        }]),
        ..Default::default()
    })
}

fn tool_msg(call_id: &str, content: Vec<ContentBlock>) -> Arc<Message> {
    Arc::new(Message {
        role: Role::Tool,
        content,
        tool_call_id: Some(call_id.into()),
        ..Default::default()
    })
}

fn image_block(url: &str) -> ContentBlock {
    ContentBlock::ImageUrl {
        image_url: ImageUrl {
            url: url.into(),
            detail: None,
        },
    }
}

fn blocks_of(msg: &OpenAIMessage) -> &[OpenAIContentBlock] {
    match &msg.content {
        OpenAIContent::Blocks(blocks) => blocks,
        OpenAIContent::Text(_) => panic!("Expected Blocks content, got {:?}", msg.content),
    }
}

#[test]
fn test_convert_tool_message_with_image_moves_image_to_user_message() {
    let messages = vec![
        assistant_with_tool_call("call_1"),
        tool_msg(
            "call_1",
            vec![
                ContentBlock::Text {
                    text: "screenshot captured".into(),
                },
                image_block("data:image/png;base64,QUJD"),
            ],
        ),
    ];

    let converted = OpenAIProvider::convert_messages(&messages);
    assert_eq!(converted.len(), 3);
    assert_eq!(converted[0].role, "assistant");

    // Tool message keeps only text; the API rejects image_url in tool messages
    assert_eq!(converted[1].role, "tool");
    let blocks = blocks_of(&converted[1]);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].type_, "text");
    assert!(blocks[0].image_url.is_none());

    // The image is flushed as a trailing user message
    assert_eq!(converted[2].role, "user");
    assert!(converted[2].tool_call_id.is_none());
    assert!(converted[2].tool_calls.is_none());
    assert!(converted[2].reasoning_content.is_none());
    let blocks = blocks_of(&converted[2]);
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].type_, "text");
    assert_eq!(
        blocks[0].text.as_deref(),
        Some("[Images from preceding tool results]")
    );
    assert_eq!(blocks[1].type_, "image_url");
    assert_eq!(
        blocks[1].image_url.as_ref().unwrap().url,
        "data:image/png;base64,QUJD"
    );
}

#[test]
fn test_convert_consecutive_tool_messages_flush_single_user_message() {
    let messages = vec![
        assistant_with_tool_call("call_1"),
        tool_msg(
            "call_1",
            vec![
                ContentBlock::Text { text: "one".into() },
                image_block("https://example.com/a.png"),
            ],
        ),
        tool_msg(
            "call_2",
            vec![
                ContentBlock::Text { text: "two".into() },
                image_block("data:image/png;base64,QUJD"),
            ],
        ),
    ];

    let converted = OpenAIProvider::convert_messages(&messages);
    // assistant, tool, tool, user — the tool run stays contiguous
    assert_eq!(converted.len(), 4);
    assert_eq!(converted[0].role, "assistant");
    assert_eq!(converted[1].role, "tool");
    assert_eq!(converted[2].role, "tool");
    assert_eq!(converted[3].role, "user");

    let blocks = blocks_of(&converted[3]);
    assert_eq!(blocks.len(), 3, "header text + 2 images");
    assert_eq!(
        blocks[1].image_url.as_ref().unwrap().url,
        "https://example.com/a.png"
    );
    assert_eq!(
        blocks[2].image_url.as_ref().unwrap().url,
        "data:image/png;base64,QUJD"
    );
}

#[test]
fn test_convert_tool_message_without_image_unchanged() {
    let messages = vec![
        assistant_with_tool_call("call_1"),
        tool_msg(
            "call_1",
            vec![ContentBlock::Text {
                text: "plain output".into(),
            }],
        ),
    ];

    let converted = OpenAIProvider::convert_messages(&messages);
    assert_eq!(converted.len(), 2);
    assert_eq!(converted[1].role, "tool");
    let blocks = blocks_of(&converted[1]);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text.as_deref(), Some("plain output"));
}

#[test]
fn test_convert_tool_image_flushed_before_next_non_tool_message() {
    let messages = vec![
        assistant_with_tool_call("call_1"),
        tool_msg("call_1", vec![image_block("data:image/png;base64,QUJD")]),
        Arc::new(Message::assistant("done")),
    ];

    let converted = OpenAIProvider::convert_messages(&messages);
    // assistant, tool, user(images), assistant — flush lands right after the
    // tool run, before the following non-tool message
    assert_eq!(converted.len(), 4);
    assert_eq!(converted[0].role, "assistant");
    assert_eq!(converted[1].role, "tool");
    assert_eq!(converted[2].role, "user");
    assert_eq!(converted[3].role, "assistant");

    // The image-only tool message got a placeholder text
    let blocks = blocks_of(&converted[1]);
    assert_eq!(blocks.len(), 1);
    assert_eq!(
        blocks[0].text.as_deref(),
        Some("[image(s) attached in the following user message]")
    );
}

#[test]
fn test_convert_flushes_pending_images_before_following_user_message() {
    let messages = vec![
        assistant_with_tool_call("call_1"),
        tool_msg("call_1", vec![image_block("data:image/png;base64,QUJD")]),
        Arc::new(Message::user("next question")),
    ];

    let converted = OpenAIProvider::convert_messages(&messages);
    assert_eq!(converted.len(), 4);
    assert_eq!(converted[1].role, "tool");
    assert_eq!(converted[2].role, "user");
    assert!(blocks_of(&converted[2])
        .iter()
        .any(|b| b.type_ == "image_url"));
    assert_eq!(converted[3].role, "user");
    assert!(
        blocks_of(&converted[3]).iter().all(|b| b.type_ == "text"),
        "original user message must not gain images"
    );
}

#[test]
fn test_convert_internal_message_does_not_break_tool_run() {
    let messages = vec![
        Arc::new(Message {
            role: Role::Assistant,
            content: vec![],
            tool_calls: Some(vec![
                ToolCall {
                    id: "call_1".into(),
                    name: "a".into(),
                    arguments: serde_json::json!({}),
                },
                ToolCall {
                    id: "call_2".into(),
                    name: "b".into(),
                    arguments: serde_json::json!({}),
                },
            ]),
            ..Default::default()
        }),
        tool_msg("call_1", vec![ContentBlock::Text { text: "one".into() }]),
        Arc::new(Message {
            role: Role::Internal,
            content: vec![ContentBlock::Text {
                text: "internal note".into(),
            }],
            ..Default::default()
        }),
        tool_msg("call_2", vec![image_block("data:image/png;base64,QUJD")]),
    ];

    let converted = OpenAIProvider::convert_messages(&messages);
    // Internal 被过滤且不打断 tool run：assistant, tool, tool, flushed user。
    assert_eq!(converted.len(), 4);
    assert_eq!(converted[1].role, "tool");
    assert_eq!(converted[2].role, "tool");
    assert_eq!(converted[3].role, "user");
    assert!(blocks_of(&converted[3])
        .iter()
        .any(|b| b.type_ == "image_url"));
}

#[test]
fn test_convert_two_tool_runs_each_flush_their_own_images() {
    let messages = vec![
        assistant_with_tool_call("call_1"),
        tool_msg("call_1", vec![image_block("data:image/png;base64,QQ==")]),
        Arc::new(Message::user("between runs")),
        assistant_with_tool_call("call_2"),
        tool_msg("call_2", vec![image_block("data:image/png;base64,Qg==")]),
    ];

    let converted = OpenAIProvider::convert_messages(&messages);
    let roles: Vec<&str> = converted.iter().map(|m| m.role.as_str()).collect();
    assert_eq!(
        roles,
        [
            "assistant",
            "tool",
            "user",
            "user",
            "assistant",
            "tool",
            "user"
        ]
    );
    // 第一段 run 的 flush 只含第一张图，第二段 flush 只含第二张。
    let first = blocks_of(&converted[2]);
    let last = blocks_of(&converted[6]);
    assert_eq!(
        first[1].image_url.as_ref().unwrap().url,
        "data:image/png;base64,QQ=="
    );
    assert_eq!(
        last[1].image_url.as_ref().unwrap().url,
        "data:image/png;base64,Qg=="
    );
}

#[test]
fn test_convert_empty_tool_message_gets_no_output_placeholder() {
    let messages = vec![
        assistant_with_tool_call("call_1"),
        tool_msg("call_1", vec![]),
    ];

    let converted = OpenAIProvider::convert_messages(&messages);
    assert_eq!(converted.len(), 2);
    let blocks = blocks_of(&converted[1]);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text.as_deref(), Some("(no output)"));
}
