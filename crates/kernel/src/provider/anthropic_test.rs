use super::*;

use crate::types::{AudioData, ToolCall};
use chrono::Utc;

#[test]
fn test_extract_system_message() {
    let messages: Vec<Arc<Message>> = vec![
        Arc::new(Message::system("You are a helpful assistant")),
        Arc::new(Message::user("Hello")),
    ];

    let system = AnthropicProvider::extract_system_message(&messages);
    assert_eq!(system, Some("You are a helpful assistant".to_string()));
}

#[test]
fn test_convert_messages_filters_system() {
    let messages: Vec<Arc<Message>> = vec![
        Arc::new(Message::system("System prompt")),
        Arc::new(Message::user("Hello")),
        Arc::new(Message::assistant("Hi there")),
    ];

    let converted = AnthropicProvider::convert_messages(&messages);
    assert_eq!(converted.len(), 2);
    assert_eq!(converted[0].role, "user");
    assert_eq!(converted[1].role, "assistant");
}

#[test]
fn test_stream_state_text_content() {
    let mut state = AnthropicStreamState::new();

    // Simulate content block start
    let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Hello"}}"#;
    let items = state.process(event).unwrap();
    assert!(items.is_empty());

    // Simulate delta
    let event =
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#;
    let items = state.process(event).unwrap();
    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::Chunk(ContentChunk::Text(text)) => {
            assert_eq!(text, " world");
        }
        _ => panic!("Expected text chunk"),
    }
}

#[test]
fn test_stream_state_thinking_content() {
    let mut state = AnthropicStreamState::new();

    let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#;
    let items = state.process(event).unwrap();

    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::Chunk(ContentChunk::Thinking { thinking, .. }) => {
            assert_eq!(thinking, "Let me think...");
        }
        _ => panic!("Expected thinking chunk"),
    }
}

#[test]
fn test_stream_state_tool_use() {
    let mut state = AnthropicStreamState::new();

    // Tool use starts
    let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_123","name":"bash","input":{}}}"#;
    let items = state.process(event).unwrap();
    assert!(items.is_empty());

    // Input JSON delta - emits ToolCallDelta for UI feedback
    let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#;
    let items = state.process(event).unwrap();
    assert_eq!(items.len(), 1);
    assert!(
        matches!(&items[0], ModelStreamItem::ToolCallDelta { id, arguments_delta, .. } if id == "tool_123" && arguments_delta == "{\"cmd\":\"ls\"}")
    );

    // Content block stop - should emit tool call
    let event = r#"{"type":"content_block_stop","index":0}"#;
    let items = state.process(event).unwrap();

    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::ToolCall(call) => {
            assert_eq!(call.id, "tool_123");
            assert_eq!(call.name, "bash");
            assert_eq!(call.arguments, serde_json::json!({"cmd":"ls"}));
        }
        _ => panic!("Expected tool call"),
    }
}

#[test]
fn test_stream_state_message_stop() {
    let mut state = AnthropicStreamState::new();

    let event = r#"{"type":"message_stop"}"#;
    let items = state.process(event).unwrap();

    assert_eq!(items.len(), 1);
    assert!(matches!(items[0], ModelStreamItem::Complete));
}

#[test]
fn test_convert_tools() {
    use std::sync::Arc;
    let tools: Vec<Arc<ToolDefinition>> = vec![Arc::new(ToolDefinition {
        name: "bash".to_string(),
        description: "Execute bash commands".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "cmd": {"type": "string"}
            }
        }),
    })];

    let converted = AnthropicProvider::convert_tools(&tools);
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].name, "bash");
    assert_eq!(converted[0].description, "Execute bash commands");
}

#[test]
fn test_convert_tool_result_message() {
    // Create a tool result message
    let messages: Vec<Arc<Message>> = vec![Arc::new(Message {
        role: Role::Tool,
        content: vec![ContentBlock::Text {
            text: "File contents here".to_string(),
        }],
        tool_calls: None,
        tool_call_id: Some("tool_123".to_string()),
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    })];

    let converted = AnthropicProvider::convert_messages(&messages);
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].role, "user");
    assert_eq!(converted[0].content.len(), 1);

    // Check that it's a ToolResult content block
    match &converted[0].content[0] {
        AnthropicContent::ToolResult {
            tool_use_id,
            content,
        } => {
            assert_eq!(tool_use_id, "tool_123");
            assert_eq!(content, "File contents here");
        }
        _ => panic!(
            "Expected ToolResult content block, got {:?}",
            converted[0].content[0]
        ),
    }

    // Verify JSON serialization has correct field names
    let json = serde_json::to_string(&converted[0].content[0]).unwrap();
    assert!(
        json.contains("tool_use_id"),
        "JSON should contain 'tool_use_id' field, got: {json}"
    );
    assert!(
        json.contains("tool_123"),
        "JSON should contain the tool ID, got: {json}"
    );
    assert!(
        json.contains("\"type\":\"tool_result\""),
        "JSON should have correct type, got: {json}"
    );
}

#[test]
fn test_convert_assistant_with_tool_calls() {
    // Create an assistant message with tool_calls
    let messages: Vec<Arc<Message>> = vec![Arc::new(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "I'll check that file for you.".to_string(),
        }],
        tool_calls: Some(vec![ToolCall {
            id: "tool_456".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        }]),
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    })];

    let converted = AnthropicProvider::convert_messages(&messages);
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0].role, "assistant");
    assert_eq!(converted[0].content.len(), 2); // Text + ToolUse

    // First block should be text
    match &converted[0].content[0] {
        AnthropicContent::Text { text } => {
            assert_eq!(text, "I'll check that file for you.");
        }
        _ => panic!("Expected Text content block"),
    }

    // Second block should be ToolUse
    match &converted[0].content[1] {
        AnthropicContent::ToolUse { id, name, input } => {
            assert_eq!(id, "tool_456");
            assert_eq!(name, "read");
            assert_eq!(input, &serde_json::json!({"path": "/tmp/test.txt"}));
        }
        _ => panic!("Expected ToolUse content block"),
    }
}

#[test]
fn test_convert_redacted_thinking() {
    let blocks = vec![ContentBlock::RedactedThinking {
        data: "redacted_data_123".to_string(),
    }];

    let converted = AnthropicProvider::convert_content_blocks(&blocks);
    assert_eq!(converted.len(), 1);

    match &converted[0] {
        AnthropicContent::RedactedThinking { data } => {
            assert_eq!(data, "redacted_data_123");
        }
        _ => panic!("Expected RedactedThinking content block"),
    }
}

#[test]
fn test_convert_thinking_preserved() {
    let blocks = vec![ContentBlock::Thinking {
        thinking: "Let me analyze this...".to_string(),
        signature: Some("sig_abc".to_string()),
    }];

    let converted = AnthropicProvider::convert_content_blocks(&blocks);
    assert_eq!(converted.len(), 1);

    match &converted[0] {
        AnthropicContent::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, "Let me analyze this...");
            assert_eq!(signature, "sig_abc");
        }
        _ => panic!("Expected Thinking content block, got {:?}", converted[0]),
    }
}

#[test]
fn test_convert_audio_skipped() {
    let blocks = vec![
        ContentBlock::Text {
            text: "Hello".to_string(),
        },
        ContentBlock::Audio {
            audio: AudioData {
                data: "base64audio".to_string(),
                format: "mp3".to_string(),
            },
        },
    ];

    let converted = AnthropicProvider::convert_content_blocks(&blocks);
    assert_eq!(converted.len(), 1);

    match &converted[0] {
        AnthropicContent::Text { text } => assert_eq!(text, "Hello"),
        _ => panic!("Expected only Text content block, audio should be skipped"),
    }
}

#[test]
fn test_multi_turn_tool_conversation() {
    // Simulate a full tool use conversation flow
    let messages: Vec<Arc<Message>> = vec![
        // User asks a question
        Arc::new(Message::user("What's the weather?")),
        // Assistant responds with a tool call
        Arc::new(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "I'll check the weather for you.".to_string(),
            }],
            tool_calls: Some(vec![ToolCall {
                id: "weather_1".to_string(),
                name: "get_weather".to_string(),
                arguments: serde_json::json!({"location": "New York"}),
            }]),
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
            ..Default::default()
        }),
        // Tool result
        Arc::new(Message {
            role: Role::Tool,
            content: vec![ContentBlock::Text {
                text: "72°F and sunny".to_string(),
            }],
            tool_calls: None,
            tool_call_id: Some("weather_1".to_string()),
            created_at: Utc::now(),
            token_usage: None,
            ..Default::default()
        }),
        // Final assistant response
        Arc::new(Message::assistant("It's 72°F and sunny in New York!")),
    ];

    let converted = AnthropicProvider::convert_messages(&messages);
    assert_eq!(converted.len(), 4);

    // Check user message
    assert_eq!(converted[0].role, "user");
    assert!(matches!(
        converted[0].content[0],
        AnthropicContent::Text { .. }
    ));

    // Check assistant with tool_use
    assert_eq!(converted[1].role, "assistant");
    assert_eq!(converted[1].content.len(), 2);
    assert!(matches!(
        converted[1].content[0],
        AnthropicContent::Text { .. }
    ));
    assert!(matches!(
        converted[1].content[1],
        AnthropicContent::ToolUse { .. }
    ));

    // Check tool result
    assert_eq!(converted[2].role, "user");
    assert!(matches!(
        converted[2].content[0],
        AnthropicContent::ToolResult { .. }
    ));

    // Check final assistant response
    assert_eq!(converted[3].role, "assistant");
    assert!(matches!(
        converted[3].content[0],
        AnthropicContent::Text { .. }
    ));
}

#[test]
fn test_full_request_serialization() {
    // Test the full request body serialization to catch any JSON structure issues
    use std::sync::Arc;

    let messages: Vec<Arc<Message>> = vec![
        Arc::new(Message::user("What's the weather?")),
        Arc::new(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "I'll check that for you.".to_string(),
            }],
            tool_calls: Some(vec![ToolCall {
                id: "toolu_01D7FLrfh4GYq7yT1ULFeyMV".to_string(),
                name: "get_weather".to_string(),
                arguments: serde_json::json!({"location": "NYC"}),
            }]),
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
            ..Default::default()
        }),
        Arc::new(Message::tool_result(
            crate::types::MessageId::default(),
            "toolu_01D7FLrfh4GYq7yT1ULFeyMV",
            "72°F and sunny",
        )),
    ];

    let anthropic_messages = AnthropicProvider::convert_messages(&messages);
    let request = AnthropicRequest {
        model: "claude-3-sonnet-20240229".to_string(),
        max_tokens: None,
        messages: anthropic_messages,
        system: None,
        tools: None,
        stream: true,
        temperature: None,
        thinking: None,
        output_config: None,
    };

    let json = serde_json::to_string_pretty(&request).unwrap();

    // Debug: print the JSON
    println!("Request JSON:\n{json}");

    // Verify the JSON structure (with spaces as serde_json pretty-prints)
    assert!(
        json.contains("\"type\": \"tool_use\""),
        "Should contain tool_use block"
    );
    assert!(
        json.contains("\"type\": \"tool_result\""),
        "Should contain tool_result block"
    );
    assert!(
        json.contains("\"tool_use_id\": \"toolu_01D7FLrfh4GYq7yT1ULFeyMV\""),
        "Should contain tool_use_id with correct value"
    );
    assert!(
        json.contains("\"id\": \"toolu_01D7FLrfh4GYq7yT1ULFeyMV\""),
        "Should contain tool_use id"
    );

    // Ensure no empty tool_use_id
    assert!(
        !json.contains("\"tool_use_id\": \"\""),
        "Should not contain empty tool_use_id"
    );
    assert!(
        !json.contains("\"tool_use_id\": null"),
        "Should not contain null tool_use_id"
    );
}

#[test]
fn test_output_config_serialization() {
    let request = AnthropicRequest {
        model: "claude-sonnet-4-20250514".to_string(),
        max_tokens: Some(8192),
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: vec![AnthropicContent::Text {
                text: "Hello".to_string(),
            }],
        }],
        system: None,
        tools: None,
        stream: true,
        temperature: None,
        thinking: None,
        output_config: Some(AnthropicOutputConfig {
            effort: "high".to_string(),
        }),
    };

    let json = serde_json::to_string_pretty(&request).unwrap();
    println!("Request with output_config: {json}");

    assert!(
        json.contains("\"output_config\""),
        "Should contain output_config"
    );
    assert!(
        json.contains("\"effort\": \"high\""),
        "Should contain effort: high"
    );
}

#[test]
fn test_stream_state_token_usage() {
    let mut state = AnthropicStreamState::new();

    // Simulate message_start with input_tokens (no cache)
    let event = r#"{"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","content":[],"model":"claude-3","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":100,"output_tokens":1}}}"#;
    let items = state.process(event).unwrap();
    assert!(items.is_empty()); // No items emitted on message_start

    // Simulate message_delta with output_tokens
    let event = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":0,"output_tokens":55}}"#;
    let items = state.process(event).unwrap();

    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::TokenUsage(usage) => {
            // prompt_tokens should come from message_start (100), not message_delta (0)
            assert_eq!(
                usage.prompt_tokens, 100,
                "prompt_tokens should be from message_start"
            );
            assert_eq!(
                usage.completion_tokens, 55,
                "completion_tokens should be from message_delta"
            );
            assert_eq!(usage.cached_tokens, None, "no cache tokens");
        }
        _ => panic!("Expected TokenUsage item, got {:?}", items[0]),
    }
}

#[test]
fn test_stream_state_token_usage_with_cache() {
    let mut state = AnthropicStreamState::new();

    // Simulate message_start with cache tokens
    let event = r#"{"type":"message_start","message":{"id":"msg_456","type":"message","role":"assistant","content":[],"model":"claude-3-5-sonnet","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":50,"output_tokens":0,"cache_read_input_tokens":100}}}"#;
    let items = state.process(event).unwrap();
    assert!(items.is_empty());

    // Simulate message_delta with output_tokens
    let event = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":30}}"#;
    let items = state.process(event).unwrap();

    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::TokenUsage(usage) => {
            // prompt_tokens = input_tokens + cache_read_input_tokens = 50 + 100 = 150
            assert_eq!(
                usage.prompt_tokens, 150,
                "prompt_tokens should be input_tokens + cache_read_input_tokens"
            );
            assert_eq!(
                usage.completion_tokens, 30,
                "completion_tokens should be from message_delta"
            );
            assert_eq!(
                usage.cached_tokens,
                Some(100),
                "cached_tokens should be cache_read_input_tokens"
            );
        }
        _ => panic!("Expected TokenUsage item, got {:?}", items[0]),
    }
}
