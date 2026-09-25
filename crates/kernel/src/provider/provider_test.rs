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

#[tokio::test]
async fn reqwest_error_surfaces_cause_not_url_boilerplate() {
    // Port 9 (discard) is virtually never bound; connect fails fast and
    // offline. The display must name the cause, not reqwest's
    // "error sending request for url (…)" boilerplate.
    let err = reqwest::get("http://127.0.0.1:9/").await.unwrap_err();
    let msg = ProviderError::from(err).to_string();
    assert!(!msg.contains("error sending request"), "got: {msg}");
    assert!(!msg.contains("Request failed"), "got: {msg}");
}

#[test]
fn provider_error_display_has_no_doubled_prefix() {
    // Category prefix lives either in the template or in the message, never both.
    assert_eq!(
        ProviderError::Timeout("timed out".into()).to_string(),
        "timed out"
    );
    assert_eq!(ProviderError::Request("boom".into()).to_string(), "boom");
    assert_eq!(
        ProviderError::Sse("boom".into()).to_string(),
        "SSE error: boom"
    );
}

#[test]
fn api_error_display_drops_option_code_noise() {
    // `{code:?}` would print `Some("rate_limited")`, hiding the message
    // behind debug noise on tight surfaces like status card titles.
    let err = ProviderError::Api {
        code: Some("rate_limited".into()),
        message: "Rate limit reached".into(),
        retryable: true,
    };
    assert_eq!(err.to_string(), "API error: Rate limit reached");
}

#[test]
fn root_cause_message_drills_to_deepest_source() {
    #[derive(Debug)]
    struct Wrap(
        &'static str,
        Option<Box<dyn std::error::Error + Send + Sync>>,
    );
    impl std::fmt::Display for Wrap {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Wrap {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            match &self.1 {
                Some(e) => Some(e.as_ref()),
                None => None,
            }
        }
    }
    let leaf = Wrap("connection refused", None);
    let mid = Wrap("tcp connect error", Some(Box::new(leaf)));
    let top = Wrap(
        "error sending request for url (http://x)",
        Some(Box::new(mid)),
    );
    assert_eq!(root_cause_message(&top), "connection refused");
    // No source: falls back to the error's own message.
    let bare = Wrap("weird failure", None);
    assert_eq!(root_cause_message(&bare), "weird failure");
}
