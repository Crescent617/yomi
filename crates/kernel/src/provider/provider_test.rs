use super::*;
use crate::types::Message;
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
fn resolve_request_config_rejects_explicit_zero_max_tokens() {
    let messages = vec![Arc::new(Message::user("hello"))];
    let error = resolve_request_config(&messages, &[], &config(100_000, Some(0)))
        .expect_err("zero max_tokens must not reach a provider");
    assert!(matches!(error, ProviderError::Config(_)));
}

#[test]
fn parse_retry_after_reads_seconds_form() {
    use reqwest::header::{HeaderMap, RETRY_AFTER};

    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, "12".parse().unwrap());
    assert_eq!(
        super::parse_retry_after(&headers),
        Some(std::time::Duration::from_secs(12))
    );

    // Non-numeric forms (e.g. HTTP-date) are ignored.
    headers.insert(
        RETRY_AFTER,
        "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
    );
    assert_eq!(super::parse_retry_after(&headers), None);

    assert_eq!(super::parse_retry_after(&HeaderMap::new()), None);
}

#[test]
fn http_error_carries_retry_after_into_provider_error() {
    let error = ProviderError::Http(HttpError::new(
        429,
        Some(std::time::Duration::from_secs(30)),
    ));
    assert!(error.is_retryable());
    assert_eq!(
        error.retry_after(),
        Some(std::time::Duration::from_secs(30))
    );

    let other = ProviderError::Timeout("stall".into());
    assert_eq!(other.retry_after(), None);
}
