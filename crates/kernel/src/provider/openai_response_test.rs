use super::*;
use crate::types::{ImageUrl, ToolCall};

// ==== helpers ====

fn user_msg(text: &str) -> Arc<Message> {
    Arc::new(Message {
        role: Role::User,
        content: vec![ContentBlock::Text { text: text.into() }],
        ..Default::default()
    })
}

fn system_msg(text: &str) -> Arc<Message> {
    Arc::new(Message {
        role: Role::System,
        content: vec![ContentBlock::Text { text: text.into() }],
        ..Default::default()
    })
}

// ==== message conversion ====

#[test]
fn test_extract_instructions() {
    let messages = vec![
        system_msg("You are helpful."),
        user_msg("hi"),
        system_msg("Be concise."),
    ];
    let instructions = OpenAIResponseProvider::extract_instructions(&messages);
    assert_eq!(
        instructions.as_deref(),
        Some("You are helpful.\n\nBe concise.")
    );

    let messages = vec![user_msg("hi")];
    assert!(OpenAIResponseProvider::extract_instructions(&messages).is_none());
}

#[test]
fn test_convert_messages_system_excluded() {
    let messages = vec![system_msg("sys"), user_msg("hello")];
    let items = OpenAIResponseProvider::convert_messages(&messages);
    assert_eq!(items.len(), 1);
    match &items[0] {
        InputItem::Message { role, content } => {
            assert_eq!(role, "user");
            assert_eq!(content.len(), 1);
            assert!(matches!(&content[0], ContentPart::InputText { text } if text == "hello"));
        }
        other => panic!("Expected user message, got {other:?}"),
    }
}

#[test]
fn test_convert_messages_internal_filtered() {
    let messages = vec![
        Arc::new(Message {
            role: Role::Internal,
            content: vec![ContentBlock::Text {
                text: "internal".into(),
            }],
            ..Default::default()
        }),
        user_msg("hi"),
    ];
    let items = OpenAIResponseProvider::convert_messages(&messages);
    assert_eq!(items.len(), 1);
}

#[test]
fn test_convert_assistant_with_tool_calls() {
    let messages = vec![Arc::new(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "Let me check.".into(),
        }],
        tool_calls: Some(vec![ToolCall {
            id: "call_abc".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"cmd": "ls"}),
        }]),
        ..Default::default()
    })];

    let items = OpenAIResponseProvider::convert_messages(&messages);
    assert_eq!(items.len(), 2);
    assert!(matches!(
        &items[0],
        InputItem::Message { role, .. } if role == "assistant"
    ));
    match &items[1] {
        InputItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => {
            assert_eq!(call_id, "call_abc");
            assert_eq!(name, "bash");
            let parsed: Value = serde_json::from_str(arguments).unwrap();
            assert_eq!(parsed, serde_json::json!({"cmd": "ls"}));
        }
        other => panic!("Expected FunctionCall, got {other:?}"),
    }
}

#[test]
fn test_convert_tool_result() {
    let messages = vec![Arc::new(Message {
        role: Role::Tool,
        content: vec![ContentBlock::Text {
            text: "file1\nfile2".into(),
        }],
        tool_call_id: Some("call_abc".into()),
        ..Default::default()
    })];

    let items = OpenAIResponseProvider::convert_messages(&messages);
    assert_eq!(items.len(), 1);
    match &items[0] {
        InputItem::FunctionCallOutput { call_id, output } => {
            assert_eq!(call_id, "call_abc");
            assert_eq!(output, "file1\nfile2");
        }
        other => panic!("Expected FunctionCallOutput, got {other:?}"),
    }
}

#[test]
fn test_convert_tool_result_without_call_id_skipped() {
    let messages = vec![Arc::new(Message {
        role: Role::Tool,
        content: vec![ContentBlock::Text {
            text: "orphan".into(),
        }],
        tool_call_id: None,
        ..Default::default()
    })];
    let items = OpenAIResponseProvider::convert_messages(&messages);
    assert!(items.is_empty());
}

#[test]
fn test_convert_user_image() {
    let messages = vec![Arc::new(Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text {
                text: "what is this?".into(),
            },
            ContentBlock::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AAAA".into(),
                    detail: Some("high".into()),
                },
            },
        ],
        ..Default::default()
    })];

    let items = OpenAIResponseProvider::convert_messages(&messages);
    assert_eq!(items.len(), 1);
    match &items[0] {
        InputItem::Message { content, .. } => {
            assert_eq!(content.len(), 2);
            assert!(matches!(&content[0], ContentPart::InputText { .. }));
            match &content[1] {
                ContentPart::InputImage { image_url, detail } => {
                    assert_eq!(image_url, "data:image/png;base64,AAAA");
                    assert_eq!(detail.as_deref(), Some("high"));
                }
                other => panic!("Expected InputImage, got {other:?}"),
            }
        }
        other => panic!("Expected Message, got {other:?}"),
    }
}

#[test]
fn test_convert_thinking_block_dropped() {
    let messages = vec![Arc::new(Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                thinking: "hmm...".into(),
                signature: None,
            },
            ContentBlock::Text {
                text: "answer".into(),
            },
        ],
        ..Default::default()
    })];

    let items = OpenAIResponseProvider::convert_messages(&messages);
    assert_eq!(items.len(), 1);
    match &items[0] {
        InputItem::Message { content, .. } => {
            // Only the text remains; thinking is dropped (Phase 1)
            assert_eq!(content.len(), 1);
            assert!(matches!(&content[0], ContentPart::OutputText { text } if text == "answer"));
        }
        other => panic!("Expected Message, got {other:?}"),
    }
}

#[test]
fn test_convert_tools_flat_format() {
    let tools = vec![Arc::new(ToolDefinition {
        name: "read".into(),
        description: "Read a file".into(),
        parameters: serde_json::json!({"type": "object"}),
    })];
    let converted = OpenAIResponseProvider::convert_tools(&tools);
    assert_eq!(converted.len(), 1);

    // Verify serialized wire format is flat (no nested "function" object)
    let json = serde_json::to_value(&converted[0]).unwrap();
    assert_eq!(json["type"], "function");
    assert_eq!(json["name"], "read");
    assert_eq!(json["description"], "Read a file");
    assert!(json.get("function").is_none());
}

// ==== request serialization ====

#[test]
fn test_input_item_wire_format() {
    let item = InputItem::FunctionCallOutput {
        call_id: "call_1".into(),
        output: "ok".into(),
    };
    let json = serde_json::to_value(&item).unwrap();
    assert_eq!(json["type"], "function_call_output");
    assert_eq!(json["call_id"], "call_1");
    assert_eq!(json["output"], "ok");

    let item = InputItem::Message {
        role: "user".into(),
        content: vec![ContentPart::InputText { text: "hi".into() }],
    };
    let json = serde_json::to_value(&item).unwrap();
    assert_eq!(json["type"], "message");
    assert_eq!(json["content"][0]["type"], "input_text");
}

#[test]
fn test_request_max_output_tokens_is_optional() {
    let request = ResponsesRequest {
        model: "gpt-test".into(),
        input: Vec::new(),
        instructions: None,
        tools: None,
        stream: true,
        store: false,
        max_output_tokens: None,
        temperature: None,
        reasoning: None,
    };
    let json = serde_json::to_value(request).unwrap();
    assert!(json.get("max_output_tokens").is_none());

    let request = ResponsesRequest {
        model: "gpt-test".into(),
        input: Vec::new(),
        instructions: None,
        tools: None,
        stream: true,
        store: false,
        max_output_tokens: Some(1234),
        temperature: None,
        reasoning: None,
    };
    let json = serde_json::to_value(request).unwrap();
    assert_eq!(json["max_output_tokens"], 1234);
}

// ==== assembler: text streaming ====

#[test]
fn test_assembler_text_stream() {
    let mut assembler = ResponseAssembler::new();

    let items = assembler
        .process(
            r#"{"type":"response.created","response":{"id":"resp_123","status":"in_progress"}}"#,
        )
        .unwrap();
    assert!(items.is_empty());

    let items = assembler
        .process(r#"{"type":"response.output_text.delta","output_index":0,"delta":"Hello"}"#)
        .unwrap();
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ModelStreamItem::Chunk(ContentChunk::Text(t)) if t == "Hello"
    ));

    let items = assembler
        .process(r#"{"type":"response.output_text.delta","output_index":0,"delta":" world"}"#)
        .unwrap();
    assert_eq!(items.len(), 1);

    let items = assembler
        .process(
            r#"{"type":"response.completed","response":{"id":"resp_123","status":"completed","usage":{"input_tokens":10,"output_tokens":5,"input_tokens_details":{"cached_tokens":3}}}}"#,
        )
        .unwrap();

    // TokenUsage + ResponseMeta + Complete
    assert_eq!(items.len(), 3);
    assert!(matches!(
        &items[0],
        ModelStreamItem::TokenUsage(crate::provider::TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            cached_tokens: Some(3),
        })
    ));
    assert!(matches!(
        &items[1],
        ModelStreamItem::ResponseMeta {
            response_id,
            finish_reason: Some(FinishReason::Stop),
        } if response_id.as_deref() == Some("resp_123")
    ));
    assert!(matches!(items[2], ModelStreamItem::Complete));
    assert!(assembler.done);

    // Double finish is guarded
    assert!(assembler.finish().is_empty());
}

// ==== assembler: reasoning summary ====

#[test]
fn test_assembler_reasoning_summary_delta() {
    let mut assembler = ResponseAssembler::new();
    let items = assembler
        .process(
            r#"{"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"Thinking..."}"#,
        )
        .unwrap();
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ModelStreamItem::Chunk(ContentChunk::Thinking { thinking, signature: None }) if thinking == "Thinking..."
    ));
}

// ==== assembler: tool calls ====

#[test]
fn test_assembler_single_tool_call() {
    let mut assembler = ResponseAssembler::new();

    // Tool call item added
    let items = assembler
        .process(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_123","name":"bash","arguments":""}}"#,
        )
        .unwrap();
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ModelStreamItem::ToolCallDelta { id, name, arguments_delta }
            if id == "call_123" && name == "bash" && arguments_delta.is_empty()
    ));

    // Arguments stream in
    let items = assembler
        .process(
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"cmd\":\""}"#,
        )
        .unwrap();
    assert_eq!(items.len(), 1);
    assert!(matches!(
        &items[0],
        ModelStreamItem::ToolCallDelta { arguments_delta, .. } if arguments_delta == "{\"cmd\":\""
    ));

    let items = assembler
        .process(
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"ls\"}"}"#,
        )
        .unwrap();
    assert_eq!(items.len(), 1);

    // Item done -> complete ToolCall with call_id as id
    let items = assembler
        .process(
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_123","name":"bash","arguments":"{\"cmd\":\"ls\"}"}}"#,
        )
        .unwrap();
    assert_eq!(items.len(), 1);
    match &items[0] {
        ModelStreamItem::ToolCall(call) => {
            assert_eq!(call.id, "call_123");
            assert_eq!(call.name, "bash");
            assert_eq!(call.arguments, serde_json::json!({"cmd": "ls"}));
        }
        other => panic!("Expected ToolCall, got {other:?}"),
    }

    // completed -> finish_reason synthesized as ToolCalls
    let items = assembler
        .process(r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed"}}"#)
        .unwrap();
    assert!(items.iter().any(|i| matches!(
        i,
        ModelStreamItem::ResponseMeta {
            finish_reason: Some(FinishReason::ToolCalls),
            ..
        }
    )));
}

#[test]
fn test_assembler_parallel_tool_calls() {
    let mut assembler = ResponseAssembler::new();

    let _ = assembler
        .process(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read","arguments":""}}"#,
        )
        .unwrap();
    let _ = assembler
        .process(
            r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call_2","name":"write","arguments":""}}"#,
        )
        .unwrap();

    // Interleaved argument deltas
    let _ = assembler
        .process(
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":\"a\"}"}"#,
        )
        .unwrap();
    let _ = assembler
        .process(
            r#"{"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"b\"}"}"#,
        )
        .unwrap();

    let items = assembler
        .process(
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"path\":\"a\"}"}}"#,
        )
        .unwrap();
    assert!(
        matches!(&items[0], ModelStreamItem::ToolCall(c) if c.id == "call_1" && c.arguments == serde_json::json!({"path": "a"}))
    );

    let items = assembler
        .process(
            r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","call_id":"call_2","name":"write","arguments":"{\"path\":\"b\"}"}}"#,
        )
        .unwrap();
    assert!(
        matches!(&items[0], ModelStreamItem::ToolCall(c) if c.id == "call_2" && c.arguments == serde_json::json!({"path": "b"}))
    );
}

#[test]
fn test_assembler_tool_call_from_deltas_when_done_has_no_arguments() {
    // output_item.done without arguments field -> fall back to accumulated deltas
    let mut assembler = ResponseAssembler::new();

    let _ = assembler
        .process(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_9","name":"grep","arguments":""}}"#,
        )
        .unwrap();
    let _ = assembler
        .process(
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"pattern\":\"foo\"}"}"#,
        )
        .unwrap();

    let items = assembler
        .process(
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_9","name":"grep"}}"#,
        )
        .unwrap();
    assert!(
        matches!(&items[0], ModelStreamItem::ToolCall(c) if c.arguments == serde_json::json!({"pattern": "foo"}))
    );
}

#[test]
fn test_assembler_invalid_arguments_are_parse_error() {
    let mut assembler = ResponseAssembler::new();

    let _ = assembler
        .process(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"bash","arguments":""}}"#,
        )
        .unwrap();
    let err = assembler
        .process(
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"bash","arguments":"not json"}}"#,
        )
        .unwrap_err();
    assert!(matches!(err, ProviderError::Parse(_)));
    assert!(!err.is_retryable());
}

#[test]
fn test_assembler_terminal_drops_partial_call() {
    let mut assembler = ResponseAssembler::new();

    let _ = assembler
        .process(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_x","name":"bash","arguments":""}}"#,
        )
        .unwrap();
    let _ = assembler
        .process(
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{}"}"#,
        )
        .unwrap();

    let items = assembler
        .process(r#"{"type":"response.completed","response":{"status":"completed"}}"#)
        .unwrap();
    assert_eq!(items.len(), 2);
    assert!(!items
        .iter()
        .any(|item| matches!(item, ModelStreamItem::ToolCall(_))));
    assert!(
        matches!(
            &items[0],
            ModelStreamItem::ResponseMeta {
                response_id: None,
                finish_reason: Some(FinishReason::Stop),
            }
        ),
        "unexpected terminal items: {items:?}"
    );
    assert!(matches!(items[1], ModelStreamItem::Complete));
    assert!(assembler.partial_calls.is_empty());
}

// ==== assembler: terminal states & errors ====

#[test]
fn test_assembler_incomplete_max_tokens() {
    let mut assembler = ResponseAssembler::new();
    let items = assembler
        .process(
            r#"{"type":"response.incomplete","response":{"id":"resp_2","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":100,"output_tokens":50}}}"#,
        )
        .unwrap();

    assert!(items.iter().any(|i| matches!(
        i,
        ModelStreamItem::ResponseMeta {
            finish_reason: Some(FinishReason::MaxTokens),
            ..
        }
    )));
    assert!(items.iter().any(|i| matches!(
        i,
        ModelStreamItem::TokenUsage(crate::provider::TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            cached_tokens: None,
        })
    )));
}

#[test]
fn test_assembler_idless_incomplete_propagates_finish_reason() {
    let mut assembler = ResponseAssembler::new();
    let items = assembler
        .process(
            r#"{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#,
        )
        .unwrap();

    assert!(matches!(
        &items[0],
        ModelStreamItem::ResponseMeta {
            response_id: None,
            finish_reason: Some(FinishReason::MaxTokens),
        }
    ));
    assert!(matches!(items[1], ModelStreamItem::Complete));
}

#[test]
fn test_terminal_event_type_is_authoritative() {
    let mut assembler = ResponseAssembler::new();
    let items = assembler
        .process(
            r#"{"type":"response.completed","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}"#,
        )
        .unwrap();

    assert!(matches!(
        &items[0],
        ModelStreamItem::ResponseMeta {
            response_id: None,
            finish_reason: Some(FinishReason::Stop),
        }
    ));
}

#[test]
fn test_incomplete_terminal_drops_partial_call() {
    let mut assembler = ResponseAssembler::new();
    assembler
        .process(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_x","name":"bash"}}"#,
        )
        .unwrap();
    assembler
        .process(
            r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"cmd\":"}"#,
        )
        .unwrap();

    let items = assembler
        .process(
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#,
        )
        .unwrap();

    assert!(!items
        .iter()
        .any(|item| matches!(item, ModelStreamItem::ToolCall(_))));
    assert!(items.iter().any(|item| matches!(
        item,
        ModelStreamItem::ResponseMeta {
            finish_reason: Some(FinishReason::MaxTokens),
            ..
        }
    )));
    assert!(assembler.partial_calls.is_empty());
}

#[test]
fn test_assembler_premature_end_is_retryable_and_does_not_flush_partial_call() {
    for end in ["EOF", "[DONE]"] {
        let mut assembler = ResponseAssembler::new();
        assembler
            .process(
                r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_x","name":"bash"}}"#,
            )
            .unwrap();
        assembler
            .process(
                r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"cmd\":"}"#,
            )
            .unwrap();

        let err = if end == "[DONE]" {
            assembler.process_stream_data(end).unwrap_err()
        } else {
            ResponseAssembler::unexpected_end(end)
        };
        assert!(matches!(err, ProviderError::Sse(_)));
        assert!(err.is_retryable());
        assert!(err.to_string().contains("before response.completed"));
        assert!(!assembler.finished);
        assert_eq!(assembler.partial_calls.len(), 1);
    }
}

#[test]
fn test_assembler_semantic_error_is_not_retryable() {
    let mut assembler = ResponseAssembler::new();
    let err = assembler
        .process(
            r#"{"type":"response.failed","response":{"status":"failed","error":{"code":"context_length_exceeded","message":"too much input"}}}"#,
        )
        .unwrap_err();

    assert!(!err.is_retryable());
    assert!(matches!(
        err,
        ProviderError::Api {
            code: Some(ref code),
            retryable: false,
            ..
        } if code == "context_length_exceeded"
    ));
}

#[test]
fn test_assembler_failed_event_is_error() {
    let mut assembler = ResponseAssembler::new();
    let result = assembler.process(
        r#"{"type":"response.failed","response":{"id":"resp_3","status":"failed","error":{"code":"server_error","message":"boom"}}}"#,
    );
    let err = result.unwrap_err();
    assert!(err.to_string().contains("boom"));
    assert!(err.is_retryable());
    assert!(matches!(
        err,
        ProviderError::Api {
            code: Some(ref code),
            retryable: true,
            ..
        } if code == "server_error"
    ));
}

#[test]
fn test_assembler_top_level_error_event() {
    let mut assembler = ResponseAssembler::new();
    let result = assembler
        .process(r#"{"type":"error","code":"rate_limited","message":"slow down","param":null}"#);
    let err = result.unwrap_err();
    assert!(err.to_string().contains("slow down"));
    assert!(err.is_retryable());
    assert!(matches!(
        err,
        ProviderError::Api {
            code: Some(ref code),
            retryable: true,
            ..
        } if code == "rate_limited"
    ));
}

#[test]
fn test_assembler_unknown_events_ignored() {
    let mut assembler = ResponseAssembler::new();
    for data in [
        r#"{"type":"response.in_progress","response":{"id":"resp_1","status":"in_progress"}}"#,
        r#"{"type":"response.output_text.done","output_index":0,"text":"full text"}"#,
        r#"{"type":"response.content_part.added","output_index":0,"part":{"type":"output_text","text":""}}"#,
        r#"{"type":"response.function_call_arguments.done","output_index":0,"arguments":"{}"}"#,
        r#"{"type":"response.some_future_event"}"#,
    ] {
        let items = assembler.process(data).unwrap();
        assert!(items.is_empty(), "event should be ignored: {data}");
    }
}

#[test]
fn test_assembler_invalid_json_is_parse_error() {
    let mut assembler = ResponseAssembler::new();
    let err = assembler.process("not json").unwrap_err();
    assert!(matches!(err, ProviderError::Parse(_)));
    assert!(!err.is_retryable());
}

#[test]
fn test_assembler_non_function_call_items_ignored() {
    let mut assembler = ResponseAssembler::new();
    // message / reasoning output items should not create partial calls
    let items = assembler
        .process(
            r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant"}}"#,
        )
        .unwrap();
    assert!(items.is_empty());
    let items = assembler
        .process(
            r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}"#,
        )
        .unwrap();
    assert!(items.is_empty());
    assert!(assembler.partial_calls.is_empty());
}
