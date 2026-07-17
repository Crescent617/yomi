use super::*;

use crate::provider::{ModelStream, ProviderError};
use crate::types::{MessageId, MessageTokenUsage, ToolCall, ToolDefinition};
use async_trait::async_trait;
use futures::stream;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[test]
fn test_build_continuation_summary() {
    let message = build_continuation_summary("Summary: keep the current task");
    assert!(message.starts_with("This session is being continued"));
    assert!(message.contains("Summary: keep the current task"));
    assert!(message.contains("Resume the latest unfinished task directly"));
}

#[test]
fn test_parse_summary_xml_strips_analysis_and_tags() {
    let raw = r"<analysis>draft reasoning that must not persist</analysis>
<summary>
1. Primary Request and Intent
Keep the cache-sharing prefix stable.
</summary>";

    let parsed = parse_summary_xml(raw).expect("valid summary XML");

    assert_eq!(
        parsed,
        "Summary:\n1. Primary Request and Intent\nKeep the cache-sharing prefix stable."
    );
    assert!(!parsed.contains("draft reasoning"));
    assert!(!parsed.contains("<summary>"));
}

#[test]
fn test_parse_summary_xml_accepts_plain_text_fallback() {
    let parsed = parse_summary_xml("plain summary").expect("plain text summary");
    assert_eq!(parsed, "plain summary");
}

#[test]
fn test_parse_summary_xml_rejects_empty_summary_block() {
    let error = parse_summary_xml("<analysis>draft</analysis><summary>  </summary>")
        .expect_err("empty summary must fail");
    assert!(error.to_string().contains("empty <summary>"));
}

#[test]
fn test_parse_summary_xml_rejects_malformed_blocks() {
    for raw in [
        "<analysis>draft reasoning",
        "</analysis>orphan",
        "<summary>unfinished",
        "orphan</summary>",
    ] {
        parse_summary_xml(raw).expect_err("malformed compactor XML must fail");
    }
}

#[test]
fn test_trim_oldest_context_rounds_preserves_system_and_user_boundary() {
    let messages = vec![
        Arc::new(Message::system("system")),
        Arc::new(Message::user("old")),
        Arc::new(Message::assistant("old answer")),
        Arc::new(Message::user("current")),
        Arc::new(Message::assistant("current answer")),
    ];

    let trimmed = trim_oldest_context_rounds(&messages).expect("multiple rounds can be trimmed");
    assert_eq!(trimmed[0].role, Role::System);
    assert_eq!(trimmed[1].role, Role::User);
    assert_eq!(trimmed[1].text_content(), "current");
}

#[test]
fn test_calculate_tokens_with_usage() {
    let model_config = ModelConfig::default();
    let mut assistant = Message::assistant("Let me help");
    assistant.token_usage = Some(MessageTokenUsage {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
    });
    let messages = vec![Arc::new(Message::user("Hello")), Arc::new(assistant)];

    let tokens = Compactor::calculate_tokens(&messages, &[], &model_config);
    assert_eq!(tokens, 150);
}

#[test]
fn test_calculate_tokens_includes_tools_without_usage() {
    let messages = vec![Arc::new(Message::user("hello"))];
    let tools = vec![Arc::new(ToolDefinition {
        name: "read".to_string(),
        description: "Read a file".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        estimated_tokens: 500,
    })];

    let tokens = Compactor::calculate_tokens(&messages, &tools, &ModelConfig::default());
    assert!(tokens >= 500);
}

#[test]
fn test_calculate_tokens_does_not_double_count_tools_with_usage() {
    let model_config = ModelConfig::default();
    let tools = vec![Arc::new(ToolDefinition {
        name: "large_tool".to_string(),
        description: "x".repeat(10_000),
        parameters: serde_json::json!({"type": "object"}),
        estimated_tokens: 50_000,
    })];
    let mut assistant = Message::assistant("done");
    assistant.token_usage = Some(MessageTokenUsage {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
    });
    let messages = vec![Arc::new(Message::user("request")), Arc::new(assistant)];

    assert!(Compactor::calculate_tokens(&messages, &tools, &model_config) < 50_000);
}
#[test]
fn test_compactor_defaults_split_recent_windows() {
    let compactor = Compactor::default();

    assert!(!compactor.micro_compact_enabled);
    assert_eq!(compactor.keep_recent_messages, 0);
    assert_eq!(compactor.keep_recent_tool_results, 5);
}

#[test]
fn test_micro_compact_uses_tool_result_window_only() {
    let compactor = Compactor::new(0.5, 99, 0, 1000);
    let messages = vec![Arc::new(Message::tool_result(
        MessageId::default(),
        "call",
        "Result",
    ))];

    let compacted = compactor
        .micro_compact(&messages)
        .expect("tool result should be cleared");

    assert_eq!(
        compacted[0].text_content(),
        "[Old tool result content cleared]"
    );
}

#[test]
fn test_micro_compact_clears_stale_usage_baseline() {
    let compactor = Compactor::new(0.5, 0, 1, 1000);
    let mut assistant = Message::assistant("response");
    assistant.token_usage = Some(MessageTokenUsage {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
    });
    let messages = vec![
        Arc::new(Message::tool_result(
            MessageId::default(),
            "call",
            "old result",
        )),
        Arc::new(assistant),
    ];

    let compacted = compactor
        .micro_compact(&messages)
        .expect("old tool result should be compacted");
    assert!(compacted
        .iter()
        .all(|message| message.token_usage.is_none()));
}

#[test]
fn test_micro_compact() {
    use std::sync::Arc;

    let compactor = Compactor::new(0.5, 0, 2, 1000); // threshold=100, preserve tool results in last 2 messages
    let messages: Vec<Arc<Message>> = vec![
        Arc::new(Message::user("Task 1")),
        Arc::new(Message::tool_result(
            MessageId::default(),
            "call-1",
            "Result 1",
        )), // will be cleared (index 1)
        Arc::new(Message::user("Task 2")),
        Arc::new(Message::tool_result(
            MessageId::default(),
            "call-2",
            "Result 2",
        )), // kept (index 3, in keep_recent_tool_results)
        Arc::new(Message::user("Current task")), // kept (index 4)
    ];

    let compacted = compactor.micro_compact(&messages);
    assert!(compacted.is_some());
    let new_messages = compacted.unwrap();
    // Old tool result should be cleared
    assert_eq!(
        new_messages[1].text_content(),
        "[Old tool result content cleared]"
    );
    // Recent tool result should be preserved (keep_recent_tool_results = 2)
    assert_eq!(new_messages[3].text_content(), "Result 2");
    assert_eq!(new_messages[4].text_content(), "Current task");

    // Second compaction should return None (already cleared)
    let compacted_again = compactor.micro_compact(&new_messages);
    assert!(compacted_again.is_none());
}

#[tokio::test]
async fn test_auto_compact_honors_micro_compact_config() {
    let provider: Arc<dyn Provider> = Arc::new(FixedStreamProvider {
        items: vec![
            ModelStreamItem::Chunk(crate::event::ContentChunk::Text("summary".to_string())),
            ModelStreamItem::ResponseMeta {
                response_id: Some("summary-response".to_string()),
                finish_reason: Some(crate::types::FinishReason::Stop),
            },
            ModelStreamItem::Complete,
        ],
    });
    let mut assistant = Message::assistant("using a tool");
    assistant.tool_calls = Some(vec![ToolCall {
        id: "call-1".to_string(),
        name: "read".to_string(),
        arguments: serde_json::json!({}),
    }]);
    let messages = vec![
        Arc::new(Message::user("read the file")),
        Arc::new(assistant),
        Arc::new(Message::tool_result(
            MessageId::default(),
            "call-1",
            "x".repeat(10_000),
        )),
    ];
    let mut compactor = Compactor::new(0.9, 0, 0, 1_000);
    compactor.micro_compact_enabled = true;
    let model_config = ModelConfig {
        context_window: 1_000,
        ..ModelConfig::default()
    };

    let result = compactor
        .auto_compact(&messages, &[], provider, &model_config, None)
        .await
        .expect("auto-compaction should succeed")
        .expect("micro-compaction should run");

    assert_eq!(
        result.messages[2].text_content(),
        "[Old tool result content cleared]"
    );
    assert_eq!(result.token_usage, crate::provider::TokenUsage::default());
}

#[derive(Debug)]
struct RecordingProvider {
    max_tokens: Arc<Mutex<Option<u32>>>,
}

#[async_trait]
impl Provider for RecordingProvider {
    async fn stream(
        &self,
        _messages: &[Arc<Message>],
        _tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
    ) -> Result<ModelStream, ProviderError> {
        *self.max_tokens.lock().expect("max tokens lock") = config.max_tokens;
        Ok(Box::pin(stream::iter(vec![
            Ok(ModelStreamItem::Chunk(crate::event::ContentChunk::Text(
                "summary".to_string(),
            ))),
            Ok(ModelStreamItem::ResponseMeta {
                response_id: Some("summary-response".to_string()),
                finish_reason: Some(crate::types::FinishReason::Stop),
            }),
            Ok(ModelStreamItem::Complete),
        ])))
    }

    fn name(&self) -> &'static str {
        "recording"
    }
}

#[derive(Debug)]
struct CapturingProvider {
    messages: Arc<Mutex<Vec<(Role, String)>>>,
    tool_names: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Provider for CapturingProvider {
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        _config: &ModelConfig,
    ) -> Result<ModelStream, ProviderError> {
        *self.messages.lock().expect("messages lock") = messages
            .iter()
            .map(|message| (message.role, message.text_content()))
            .collect();
        *self.tool_names.lock().expect("tool names lock") =
            tools.iter().map(|tool| tool.name.clone()).collect();
        Ok(Box::pin(stream::iter(vec![
            Ok(ModelStreamItem::Chunk(crate::event::ContentChunk::Text(
                "summary".to_string(),
            ))),
            Ok(ModelStreamItem::ResponseMeta {
                response_id: Some("summary-response".to_string()),
                finish_reason: Some(crate::types::FinishReason::Stop),
            }),
            Ok(ModelStreamItem::Complete),
        ])))
    }

    fn name(&self) -> &'static str {
        "capturing"
    }
}

#[tokio::test]
async fn test_full_compact_reuses_system_history_and_tools_prefix() {
    let captured_messages = Arc::new(Mutex::new(Vec::new()));
    let captured_tool_names = Arc::new(Mutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(CapturingProvider {
        messages: Arc::clone(&captured_messages),
        tool_names: Arc::clone(&captured_tool_names),
    });
    let tools = vec![Arc::new(ToolDefinition {
        name: "update_goal".to_string(),
        description: "Update the active goal".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        estimated_tokens: 0,
    })];
    let messages = vec![
        Arc::new(Message::system("normal agent system prompt")),
        Arc::new(Message::user("original user request")),
    ];

    let result = Compactor::default()
        .full_compact(&messages, &tools, provider, &ModelConfig::default(), None)
        .await
        .expect("full compaction should succeed");

    let compacted_summary = result.messages[0].text_content();
    assert!(compacted_summary.starts_with("This session is being continued"));
    assert!(compacted_summary.contains("summary"));
    assert!(compacted_summary.contains("Resume the latest unfinished task directly"));

    let captured = captured_messages.lock().expect("messages lock");
    assert_eq!(
        captured[0],
        (Role::System, "normal agent system prompt".to_string())
    );
    assert_eq!(
        captured[1],
        (Role::User, "original user request".to_string())
    );
    assert_eq!(captured.last().map(|item| item.0), Some(Role::User));
    assert_eq!(
        captured.last().map(|item| item.1.as_str()),
        Some(SUMMARY_PROMPT)
    );
    assert_eq!(
        *captured_tool_names.lock().expect("tool names lock"),
        vec!["update_goal".to_string()]
    );
}

#[derive(Debug)]
struct OverflowThenSuccessProvider {
    calls: AtomicUsize,
    message_counts: Mutex<Vec<usize>>,
}

#[async_trait]
impl Provider for OverflowThenSuccessProvider {
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        _tools: &[Arc<ToolDefinition>],
        _config: &ModelConfig,
    ) -> Result<ModelStream, ProviderError> {
        self.message_counts
            .lock()
            .expect("message counts lock")
            .push(messages.len());
        if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
            return Err(ProviderError::Api {
                code: Some("context_length_exceeded".to_string()),
                message: "input exceeds context window".to_string(),
                retryable: false,
            });
        }
        Ok(Box::pin(stream::iter(vec![
            Ok(ModelStreamItem::Chunk(crate::event::ContentChunk::Text(
                "summary".to_string(),
            ))),
            Ok(ModelStreamItem::ResponseMeta {
                response_id: Some("summary-response".to_string()),
                finish_reason: Some(crate::types::FinishReason::Stop),
            }),
            Ok(ModelStreamItem::Complete),
        ])))
    }

    fn name(&self) -> &'static str {
        "overflow-then-success"
    }
}

#[tokio::test]
async fn test_full_compact_trims_oldest_round_after_context_overflow() {
    let provider = Arc::new(OverflowThenSuccessProvider {
        calls: AtomicUsize::new(0),
        message_counts: Mutex::new(Vec::new()),
    });
    let messages = vec![
        Arc::new(Message::system("system")),
        Arc::new(Message::user("old")),
        Arc::new(Message::assistant("old answer")),
        Arc::new(Message::user("current")),
        Arc::new(Message::assistant("current answer")),
    ];

    let result = Compactor::default()
        .full_compact(
            &messages,
            &[],
            provider.clone(),
            &ModelConfig::default(),
            None,
        )
        .await
        .expect("overflow retry should compact successfully");

    let counts = provider.message_counts.lock().expect("message counts lock");
    assert_eq!(counts.len(), 2);
    assert!(counts[1] < counts[0]);
    assert_eq!(result.messages.len(), 1);
    assert!(result.messages[0].text_content().contains("summary"));
}

#[derive(Debug)]
struct FixedStreamProvider {
    items: Vec<ModelStreamItem>,
}

#[async_trait]
impl Provider for FixedStreamProvider {
    async fn stream(
        &self,
        _messages: &[Arc<Message>],
        _tools: &[Arc<ToolDefinition>],
        _config: &ModelConfig,
    ) -> Result<ModelStream, ProviderError> {
        Ok(Box::pin(stream::iter(
            self.items.clone().into_iter().map(Ok),
        )))
    }

    fn name(&self) -> &'static str {
        "fixed-stream"
    }
}

#[tokio::test]
async fn test_full_compact_recent_suffix_keeps_tool_batch_intact() {
    let provider: Arc<dyn Provider> = Arc::new(FixedStreamProvider {
        items: vec![
            ModelStreamItem::Chunk(crate::event::ContentChunk::Text("summary".to_string())),
            ModelStreamItem::ResponseMeta {
                response_id: Some("summary-response".to_string()),
                finish_reason: Some(crate::types::FinishReason::Stop),
            },
            ModelStreamItem::Complete,
        ],
    });
    let mut assistant = Message::assistant("using tools");
    assistant.tool_calls = Some(vec![
        ToolCall {
            id: "call-1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({}),
        },
        ToolCall {
            id: "call-2".to_string(),
            name: "grep".to_string(),
            arguments: serde_json::json!({}),
        },
    ]);
    let messages = vec![
        Arc::new(Message::system("system")),
        Arc::new(Message::user("request")),
        Arc::new(assistant),
        Arc::new(Message::tool_result(
            MessageId::default(),
            "call-1",
            "read result",
        )),
        Arc::new(Message::tool_result(
            MessageId::default(),
            "call-2",
            "grep result",
        )),
    ];

    let result = Compactor::new(0.5, 1, 5, 1_000)
        .full_compact(&messages, &[], provider, &ModelConfig::default(), None)
        .await
        .expect("full compaction should succeed");

    assert_eq!(result.messages.len(), 4);
    assert_eq!(result.messages[1].role, Role::Assistant);
    assert_eq!(result.messages[2].tool_call_id.as_deref(), Some("call-1"));
    assert_eq!(result.messages[3].tool_call_id.as_deref(), Some("call-2"));
}

#[tokio::test]
async fn test_full_compact_rejects_tool_calls() {
    let provider: Arc<dyn Provider> = Arc::new(FixedStreamProvider {
        items: vec![
            ModelStreamItem::ToolCall(crate::provider::ToolCallRequest {
                id: "call-1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({}),
            }),
            ModelStreamItem::ResponseMeta {
                response_id: Some("tool-summary".to_string()),
                finish_reason: Some(crate::types::FinishReason::Stop),
            },
            ModelStreamItem::Complete,
        ],
    });

    let error = Compactor::default()
        .full_compact(
            &[Arc::new(Message::user("preserve me"))],
            &[],
            provider,
            &ModelConfig::default(),
            None,
        )
        .await
        .expect_err("summary tool calls must fail compaction");
    assert!(error.to_string().contains("attempted to call a tool"));
}

#[tokio::test]
async fn test_full_compact_rejects_truncated_summary() {
    let provider: Arc<dyn Provider> = Arc::new(FixedStreamProvider {
        items: vec![
            ModelStreamItem::Chunk(crate::event::ContentChunk::Text("partial".to_string())),
            ModelStreamItem::ResponseMeta {
                response_id: Some("truncated-summary".to_string()),
                finish_reason: Some(crate::types::FinishReason::MaxTokens),
            },
            ModelStreamItem::Complete,
        ],
    });

    let error = Compactor::default()
        .full_compact(
            &[Arc::new(Message::user("preserve me"))],
            &[],
            provider,
            &ModelConfig::default(),
            None,
        )
        .await
        .expect_err("truncated summary must not replace history");

    assert!(error.to_string().contains("MaxTokens"));
}

#[tokio::test]
async fn test_full_compact_rejects_non_final_anthropic_terminals() {
    for finish_reason in [
        crate::types::FinishReason::PauseTurn,
        crate::types::FinishReason::Refusal,
    ] {
        let provider: Arc<dyn Provider> = Arc::new(FixedStreamProvider {
            items: vec![
                ModelStreamItem::Chunk(crate::event::ContentChunk::Text("partial".to_string())),
                ModelStreamItem::ResponseMeta {
                    response_id: Some("non-final-summary".to_string()),
                    finish_reason: Some(finish_reason),
                },
                ModelStreamItem::Complete,
            ],
        });

        let error = Compactor::default()
            .full_compact(
                &[Arc::new(Message::user("preserve me"))],
                &[],
                provider,
                &ModelConfig::default(),
                None,
            )
            .await
            .expect_err("non-final summary must not replace history");

        assert!(error.to_string().contains(&format!("{finish_reason:?}")));
    }
}

#[tokio::test]
async fn test_full_compact_rejects_empty_summary() {
    let provider: Arc<dyn Provider> = Arc::new(FixedStreamProvider {
        items: vec![
            ModelStreamItem::ResponseMeta {
                response_id: Some("empty-summary".to_string()),
                finish_reason: Some(crate::types::FinishReason::Stop),
            },
            ModelStreamItem::Complete,
        ],
    });

    let error = Compactor::default()
        .full_compact(
            &[Arc::new(Message::user("preserve me"))],
            &[],
            provider,
            &ModelConfig::default(),
            None,
        )
        .await
        .expect_err("empty summary must not replace history");

    assert!(error.to_string().contains("empty summary"));
}

#[tokio::test]
async fn test_full_compact_uses_configured_summary_max_tokens() {
    let max_tokens = Arc::new(Mutex::new(None));
    let provider: Arc<dyn Provider> = Arc::new(RecordingProvider {
        max_tokens: Arc::clone(&max_tokens),
    });
    let compactor = Compactor::new(0.5, 0, 5, 1_234);
    let messages = vec![Arc::new(Message::user("compact me"))];

    compactor
        .full_compact(&messages, &[], provider, &ModelConfig::default(), None)
        .await
        .expect("full compaction should succeed");

    assert_eq!(*max_tokens.lock().expect("max tokens lock"), Some(1_234));
}

#[test]
fn test_threshold_triggers_with_33k_remaining_for_200k_context() {
    let compactor = Compactor::new(0.9, 0, 5, 8_192);

    assert_eq!(compactor.threshold(200_000), 167_000);
}

#[test]
fn test_threshold_reserves_summary_context() {
    let compactor = Compactor::new(0.9, 0, 5, 8_192);
    let threshold = compactor.threshold(32_768);
    let prompt_tokens = crate::utils::tokens::estimate_tokens(SUMMARY_PROMPT) as u32;

    assert!(
        threshold + CONTEXT_SAFETY_BUFFER_TOKENS + prompt_tokens + MIN_SUMMARY_OUTPUT_TOKENS
            <= 32_768
    );
}

#[test]
fn test_threshold_does_not_collapse_for_small_context() {
    let compactor = Compactor::new(0.9, 0, 5, 8_192);
    assert_eq!(compactor.threshold(4_000), 3_600);
}
