use super::*;
use crate::types::{ContentBlock, Message};
use std::sync::Arc;

fn config(context_window: u32, max_tokens: Option<u32>) -> ModelConfig {
    ModelConfig {
        max_tokens,
        context_window,
        ..ModelConfig::default()
    }
}

#[test]
fn provider_error_context_overflow_classification() {
    assert!(ProviderError::Api {
        code: Some("context_length_exceeded".into()),
        message: "too long".into(),
        retryable: false,
    }
    .is_context_overflow());
    assert!(ProviderError::Parse("context window exceeded".into()).is_context_overflow());
    assert!(ProviderError::Parse("maximum context length is 128k".into()).is_context_overflow());
    assert!(
        ProviderError::Parse("input tokens exceed the model's maximum".into())
            .is_context_overflow()
    );
    assert!(!ProviderError::Api {
        code: Some("server_error".into()),
        message: "boom".into(),
        retryable: true,
    }
    .is_context_overflow());
}

#[test]
fn resolve_request_config_uses_default_and_tool_estimate() {
    let messages = vec![Arc::new(Message::user("hello"))];
    let tools = vec![Arc::new(ToolDefinition {
        name: "read".into(),
        description: "Read a file".into(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        estimated_tokens: 123,
    })];

    let model_config = config(100_000, None);
    let resolved = resolve_request_config(&messages, &tools, &model_config)
        .expect("enough context should remain");
    assert_eq!(resolved.max_tokens, Some(DEFAULT_MAX_OUTPUT_TOKENS));
    assert!(estimate_request_input_tokens(&messages, &tools, &model_config) >= 123);
}

#[test]
fn resolve_request_config_caps_custom_value_to_remaining_context() {
    let messages = vec![Arc::new(Message::user("x".repeat(3_000)))];
    let model_config = config(10_000, Some(9_000));
    let input_tokens = estimate_request_input_tokens(&messages, &[], &model_config);
    let resolved = resolve_request_config(&messages, &[], &model_config)
        .expect("some output context should remain");
    assert_eq!(
        resolved.max_tokens,
        Some(10_000 - input_tokens - CONTEXT_SAFETY_BUFFER_TOKENS)
    );
}

#[test]
fn resolve_request_config_errors_when_safety_buffer_exhausts_context() {
    let messages = vec![Arc::new(Message::user("x".repeat(30_000)))];
    let error = resolve_request_config(&messages, &[], &config(10_000, None))
        .expect_err("request must not be sent without output space");
    assert!(matches!(error, ProviderError::Config(_)));
}

#[test]
fn actual_usage_baseline_uses_latest_assistant_usage() {
    let model_config = ModelConfig::default();
    let mut message = Message::assistant("response");
    message.token_usage = Some(crate::types::MessageTokenUsage {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
    });
    let messages = vec![
        Arc::new(Message::user("request")),
        Arc::new(message),
        Arc::new(Message::user("next")),
    ];
    let estimated = estimate_request_input_tokens(&messages, &[], &model_config);
    assert!(estimated > 150);
    assert!(estimated < 200);
}

#[test]
fn estimate_message_handles_non_text_content() {
    let message = Message::with_blocks(
        Role::User,
        vec![ContentBlock::ImageUrl {
            image_url: crate::types::ImageUrl {
                url: "https://example.com/image.png".into(),
                detail: None,
            },
        }],
    );
    assert!(
        estimate_request_input_tokens(&[Arc::new(message)], &[], &ModelConfig::default()) >= 4_096
    );
}

#[test]
fn internal_messages_are_not_counted() {
    let model_config = ModelConfig::default();
    let mut internal = Message::user("x".repeat(40_000));
    internal.role = Role::Internal;
    let with_internal = vec![Arc::new(Message::user("request")), Arc::new(internal)];
    let without_internal = vec![Arc::new(Message::user("request"))];

    assert_eq!(
        estimate_request_input_tokens(&with_internal, &[], &model_config),
        estimate_request_input_tokens(&without_internal, &[], &model_config)
    );
}

#[test]
fn anthropic_thinking_budget_must_fit_resolved_output() {
    let model_config = ModelConfig {
        provider: crate::config::ModelProvider::Anthropic,
        context_window: 6_000,
        thinking: ThinkingConfig {
            enabled: true,
            budget_tokens: 2_048,
            effort: None,
        },
        ..ModelConfig::default()
    };
    let error = resolve_request_config(
        &[Arc::new(Message::user("x".repeat(4_000)))],
        &[],
        &model_config,
    )
    .expect_err("thinking budget must fit resolved max_tokens");
    assert!(matches!(error, ProviderError::Config(_)));
}

#[test]
fn inline_image_estimate_scales_with_payload_size() {
    let small = Message::with_blocks(
        Role::User,
        vec![ContentBlock::ImageUrl {
            image_url: crate::types::ImageUrl {
                url: "data:image/png;base64,AAAA".into(),
                detail: None,
            },
        }],
    );
    let large = Message::with_blocks(
        Role::User,
        vec![ContentBlock::ImageUrl {
            image_url: crate::types::ImageUrl {
                url: format!("data:image/png;base64,{}", "A".repeat(40_000)),
                detail: None,
            },
        }],
    );
    let model_config = ModelConfig::default();

    assert!(
        estimate_request_input_tokens(&[Arc::new(large)], &[], &model_config)
            > estimate_request_input_tokens(&[Arc::new(small)], &[], &model_config)
    );
}

#[test]
fn resolve_request_config_rejects_explicit_zero_max_tokens() {
    let messages = vec![Arc::new(Message::user("hello"))];
    let error = resolve_request_config(&messages, &[], &config(100_000, Some(0)))
        .expect_err("zero max_tokens must not reach a provider");
    assert!(matches!(error, ProviderError::Config(_)));
}
