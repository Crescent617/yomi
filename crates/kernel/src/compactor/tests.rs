use super::*;

use crate::provider::{ModelStream, ProviderError};
use crate::types::{MessageId, MessageTokenUsage, ToolDefinition};
use async_trait::async_trait;
use futures::stream;
use std::sync::{Arc, Mutex};

#[test]
fn test_calculate_tokens_with_usage() {
    let messages: Vec<Arc<Message>> = vec![
        Arc::new(Message::user("Hello")),
        Arc::new(Message::assistant("Hi there")),
        {
            let mut msg = Message::assistant("Let me help");
            msg.token_usage = Some(MessageTokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            });
            Arc::new(msg)
        },
    ];

    let tokens = Compactor::calculate_tokens(&messages);
    // Should use the actual usage (150) plus estimation for messages after
    assert!(tokens >= 150);
}

#[test]
fn test_compactor_defaults_split_recent_windows() {
    let compactor = Compactor::default();

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
        .full_compact(&messages, provider, &ModelConfig::default(), None)
        .await
        .expect("full compaction should succeed");

    assert_eq!(*max_tokens.lock().expect("max tokens lock"), Some(1_234));
}
