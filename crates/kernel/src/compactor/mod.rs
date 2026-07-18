//! Context compression for managing long conversations
//!
//! Implements two strategies:
//! 1. Micro-compaction: Clear old tool result content (fast, no API call)
//! 2. Full summarization: Use API to generate conversation summary

use crate::provider::{ModelConfig, ModelStreamItem, Provider, CONTEXT_SAFETY_BUFFER_TOKENS};
use crate::types::{ContentBlock, FinishReason, Message, Role};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Default threshold ratio to trigger compaction (90% of context window)
pub const DEFAULT_THRESHOLD_RATIO: f32 = 0.9;
/// Default number of context-window tokens to keep available before compaction.
pub const DEFAULT_COMPACTION_REMAINING_TOKENS: u32 = 25_600; // ~25k
/// Default context window size
pub const DEFAULT_CONTEXT_WINDOW: u32 = 204_800; // 200k

/// Number of recent steps when do full compaction
const KEEP_RECENT_MESSAGES: usize = 0;
/// Number of recent messages whose tool results survive micro-compaction
const KEEP_RECENT_TOOL_RESULTS: usize = 5;
/// Max tokens for summary generation
const SUMMARY_MAX_TOKENS: u32 = 10_240; // 8k tokens for summary
/// Minimum useful summary output reserved before compaction is triggered.
const MIN_SUMMARY_OUTPUT_TOKENS: u32 = 2_048;
/// Maximum retries after a provider reports that the summary input is too large.
const MAX_CONTEXT_OVERFLOW_RETRIES: usize = 3;
/// Fraction of the oldest conversation rounds removed for each overflow retry.
const CONTEXT_OVERFLOW_TRIM_PERCENT: usize = 20;
/// Summary prompt for full compaction
const SUMMARY_PROMPT: &str = include_str!("summary_prompt.txt");

/// Compaction result containing compacted messages and token usage
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub messages: Vec<Arc<Message>>,
    pub token_usage: crate::provider::TokenUsage,
}

impl CompactionResult {
    /// Create a new compaction result
    pub fn new(messages: Vec<Arc<Message>>, token_usage: crate::provider::TokenUsage) -> Self {
        Self {
            messages,
            token_usage,
        }
    }
}

/// Errors that can occur during compaction
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("Compaction was cancelled")]
    Cancelled,
    #[error("Context overflow: {0}")]
    ContextOverflow(String),
    #[error("API error: {0}")]
    Api(String),
}

impl CompactionError {
    fn is_context_overflow(&self) -> bool {
        matches!(self, Self::ContextOverflow(_))
    }
}

impl From<crate::provider::ProviderError> for CompactionError {
    fn from(error: crate::provider::ProviderError) -> Self {
        if error.is_context_overflow() {
            Self::ContextOverflow(error.to_string())
        } else {
            Self::Api(error.to_string())
        }
    }
}

fn clear_stale_token_usage(messages: &mut [Arc<Message>]) {
    for message in messages {
        if message.token_usage.is_some() {
            Arc::make_mut(message).token_usage = None;
        }
    }
}

fn trim_oldest_context_rounds(messages: &[Arc<Message>]) -> Option<Vec<Arc<Message>>> {
    let mut system = Vec::new();
    let mut rounds: Vec<Vec<Arc<Message>>> = Vec::new();
    for message in messages {
        if message.role == Role::System {
            system.push(Arc::clone(message));
        } else if message.role == Role::User || rounds.is_empty() {
            rounds.push(vec![Arc::clone(message)]);
        } else {
            rounds
                .last_mut()
                .expect("round exists")
                .push(Arc::clone(message));
        }
    }

    if rounds.len() <= 1 {
        return None;
    }
    let drop_count = (rounds.len() * CONTEXT_OVERFLOW_TRIM_PERCENT / 100).max(1);
    let keep_from = drop_count.min(rounds.len() - 1);
    let mut trimmed = system;
    trimmed.extend(rounds.into_iter().skip(keep_from).flatten());
    clear_stale_token_usage(&mut trimmed);

    let first_non_system = trimmed.iter().find(|message| message.role != Role::System);
    if first_non_system.is_some_and(|message| message.role != Role::User) {
        trimmed.insert(
            trimmed
                .iter()
                .position(|message| message.role != Role::System)
                .unwrap_or(trimmed.len()),
            Arc::new(Message::user(
                "[Earlier conversation was truncated for compaction retry.]",
            )),
        );
    }
    Some(trimmed)
}

/// Compactor for managing conversation context
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Compactor {
    /// Whether client-side micro-compaction is enabled
    pub micro_compact_enabled: bool,
    /// Ratio (0.0–1.0) of the context window at which compaction is triggered
    pub threshold_ratio: f32,
    /// Number of recent messages to preserve during full compaction
    pub keep_recent_messages: usize,
    /// Number of recent messages whose tool results survive micro-compaction
    pub keep_recent_tool_results: usize,
    /// Max tokens for summary
    pub summary_max_tokens: u32,
}

impl Default for Compactor {
    fn default() -> Self {
        Self {
            micro_compact_enabled: false,
            threshold_ratio: DEFAULT_THRESHOLD_RATIO,
            keep_recent_messages: KEEP_RECENT_MESSAGES,
            keep_recent_tool_results: KEEP_RECENT_TOOL_RESULTS,
            summary_max_tokens: SUMMARY_MAX_TOKENS,
        }
    }
}

impl Compactor {
    /// Create a new compactor with custom settings
    pub const fn new(
        threshold_ratio: f32,
        keep_recent_messages: usize,
        keep_recent_tool_results: usize,
        summary_max_tokens: u32,
    ) -> Self {
        Self {
            micro_compact_enabled: false,
            threshold_ratio,
            keep_recent_messages,
            keep_recent_tool_results,
            summary_max_tokens,
        }
    }

    fn summary_reserve(&self) -> u32 {
        CONTEXT_SAFETY_BUFFER_TOKENS
            .saturating_add(crate::utils::tokens::estimate_tokens(SUMMARY_PROMPT) as u32)
            .saturating_add(MIN_SUMMARY_OUTPUT_TOKENS.min(self.summary_max_tokens))
    }

    /// The fixed remaining-token and summary reserves are applied only when the
    /// context window can fit them. Smaller windows retain the ratio policy and
    /// let request budgeting report insufficient context explicitly.
    #[allow(clippy::cast_precision_loss)]
    pub fn threshold(&self, context_window: u32) -> u32 {
        let ratio_threshold = (context_window as f32 * self.threshold_ratio) as u32;
        let remaining_tokens_threshold = context_window
            .checked_sub(DEFAULT_COMPACTION_REMAINING_TOKENS)
            .filter(|&threshold| threshold > 0)
            .unwrap_or(u32::MAX);
        let summary_threshold = context_window
            .checked_sub(self.summary_reserve())
            .filter(|&threshold| threshold > 0)
            .unwrap_or(u32::MAX);

        ratio_threshold
            .min(remaining_tokens_threshold)
            .min(summary_threshold)
    }

    /// Calculate total tokens from message history and, when no real assistant
    /// usage exists, the tool definitions that will be sent with the request.
    /// Actual API usage already includes tools and assistant completion, so never
    /// add either again when a validated baseline is available.
    pub fn calculate_tokens(
        messages: &[Arc<Message>],
        tools: &[Arc<crate::types::ToolDefinition>],
        _model_config: &ModelConfig,
    ) -> u32 {
        crate::utils::tokens::estimate_request_input_tokens(messages, tools)
    }

    /// Check whether compaction is needed using the exact provider-facing view.
    ///
    /// Internal metadata and incomplete tool groups are removed before token
    /// estimation, so callers do not need to duplicate request sanitization.
    /// The threshold is the earliest of the configured ratio, fixed remaining
    /// context reserve, and the reserve required for a usable summary request.
    pub fn should_compact(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<crate::types::ToolDefinition>],
        model_config: &ModelConfig,
    ) -> bool {
        let provider_messages = crate::agent::MessageBuffer::sanitized_model_messages(messages);
        let estimated_tokens = Self::calculate_tokens(&provider_messages, tools, model_config);
        let threshold = self.threshold(model_config.context_window);
        let should_compact = estimated_tokens >= threshold;
        if should_compact {
            tracing::info!(
                estimated_tokens,
                threshold,
                context_window = model_config.context_window,
                threshold_ratio = self.threshold_ratio,
                remaining_tokens_reserve = DEFAULT_COMPACTION_REMAINING_TOKENS,
                summary_reserve = self.summary_reserve(),
                "auto-compaction threshold reached"
            );
        }
        should_compact
    }

    /// Try micro-compaction: clear old tool results
    /// Returns `Some(new_messages)` if compaction was performed, None otherwise
    pub fn micro_compact(&self, messages: &[Arc<Message>]) -> Option<Vec<Arc<Message>>> {
        const CLEARED_MARKER: &str = "[Old tool result content cleared]";

        let keep_start = messages.len().saturating_sub(self.keep_recent_tool_results);
        if keep_start == 0 {
            return None;
        }

        let mut modified = false;
        let mut result = Vec::with_capacity(messages.len());

        for (idx, msg) in messages.iter().enumerate() {
            if idx < keep_start
                && msg.role == Role::Tool
                && msg.content.first().is_some_and(|c| {
                    if let ContentBlock::Text { text } = c {
                        text != CLEARED_MARKER
                    } else {
                        false
                    }
                })
            {
                // Need to clear this message
                let mut new_msg = (**msg).clone();
                new_msg.content = vec![ContentBlock::Text {
                    text: CLEARED_MARKER.to_string(),
                }];
                result.push(Arc::new(new_msg));
                modified = true;
            } else {
                result.push(Arc::clone(msg));
            }
        }

        if modified {
            clear_stale_token_usage(&mut result);
            Some(result)
        } else {
            None
        }
    }

    /// Perform full compaction: generate summary using API.
    ///
    /// Returns `CompactionResult` containing messages in order: [summary] + recent
    /// Note: System messages are NOT included in the returned result - they are
    /// recreated by the agent on session restore to avoid duplication.
    ///
    /// Supports cancellation via `cancel_token`.
    pub async fn full_compact(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<crate::types::ToolDefinition>],
        provider: Arc<dyn Provider>,
        model_config: &ModelConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<CompactionResult, CompactionError> {
        // System messages stay in the summary request for prompt-cache sharing,
        // but are not returned because Agent::apply_compacted_messages preserves
        // the original system prompt separately.
        let non_system: Vec<_> = messages
            .iter()
            .filter(|message| !matches!(message.role, Role::System | Role::Internal))
            .cloned()
            .collect();

        if non_system.len() <= self.keep_recent_messages {
            return Ok(CompactionResult::new(
                non_system,
                crate::provider::TokenUsage::default(),
            ));
        }

        // Preserve complete assistant/tool batches in the recent suffix.
        let mut recent_start = non_system.len() - self.keep_recent_messages;
        if recent_start < non_system.len() && non_system[recent_start].role == Role::Tool {
            while recent_start > 0 && non_system[recent_start].role == Role::Tool {
                recent_start -= 1;
            }
            if non_system[recent_start].role != Role::Assistant {
                while recent_start < non_system.len() && non_system[recent_start].role == Role::Tool
                {
                    recent_start += 1;
                }
            }
        }
        let recent = non_system[recent_start..].to_vec();

        let mut summary_input = crate::agent::MessageBuffer::sanitized_model_messages(messages);
        let mut overflow_retries = 0;
        let (summary_text, token_usage) = loop {
            match generate_summary(
                &summary_input,
                tools,
                Arc::clone(&provider),
                model_config,
                self.summary_max_tokens,
                cancel_token.clone(),
            )
            .await
            {
                Ok(result) => break result,
                Err(error) if error.is_context_overflow() => {
                    if overflow_retries >= MAX_CONTEXT_OVERFLOW_RETRIES {
                        return Err(error);
                    }
                    let Some(trimmed) = trim_oldest_context_rounds(&summary_input) else {
                        return Err(error);
                    };
                    overflow_retries += 1;
                    tracing::warn!(
                        retry = overflow_retries,
                        previous_messages = summary_input.len(),
                        remaining_messages = trimmed.len(),
                        "summary input exceeded context window; trimming oldest context and retrying"
                    );
                    summary_input = trimmed;
                }
                Err(error) => return Err(error),
            }
        };

        // Store a continuation instruction with the durable summary so the next
        // normal turn resumes the unfinished task instead of acknowledging the
        // compaction event.
        let summary = Message::user(build_continuation_summary(&summary_text));
        // Reconstruct: summary + recent (system_msgs NOT included)
        let mut result: Vec<Arc<Message>> =
            std::iter::once(Arc::new(summary)).chain(recent).collect();
        clear_stale_token_usage(&mut result);

        Ok(CompactionResult::new(result, token_usage))
    }

    /// Auto-compact: try micro first, then full if needed.
    ///
    /// Returns `Some(CompactionResult)` if compaction was performed, `None` otherwise.
    /// Supports cancellation via `cancel_token`.
    pub async fn auto_compact(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<crate::types::ToolDefinition>],
        provider: Arc<dyn Provider>,
        model_config: &ModelConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<Option<CompactionResult>, CompactionError> {
        if !self.should_compact(messages, tools, model_config) {
            return Ok(None);
        }

        // Client-side micro-compaction rewrites old tool results and therefore
        // breaks the stable message prefix needed for prompt-cache sharing.
        if self.micro_compact_enabled {
            // Try micro-compaction first
            if let Some(after_micro) = self.micro_compact(messages) {
                // Check if micro-compaction was sufficient
                if !self.should_compact(&after_micro, tools, model_config) {
                    return Ok(Some(CompactionResult::new(
                        after_micro,
                        crate::provider::TokenUsage::default(),
                    )));
                }
                // Need full compaction on top of micro results
                return self
                    .full_compact(
                        &after_micro,
                        tools,
                        Arc::clone(&provider),
                        model_config,
                        cancel_token,
                    )
                    .await
                    .map(Some);
            }
        }

        // Cache-first path: summarize the unmodified history so the request
        // can reuse the normal agent prompt prefix.
        self.full_compact(
            messages,
            tools,
            Arc::clone(&provider),
            model_config,
            cancel_token,
        )
        .await
        .map(Some)
    }
}

/// Generate summary using API call.
/// Returns (summary, `token_usage`) or Err if cancelled or API fails.
#[allow(clippy::semicolon_if_nothing_returned)]
async fn generate_summary(
    messages: &[Arc<Message>],
    tools: &[Arc<crate::types::ToolDefinition>],
    provider: Arc<dyn Provider>,
    model_config: &ModelConfig,
    summary_max_tokens: u32,
    cancel_token: Option<CancellationToken>,
) -> Result<(String, crate::provider::TokenUsage), CompactionError> {
    use crate::agent::MessageBuffer;

    let messages = MessageBuffer::sanitized_model_messages(messages);

    // Reuse the normal conversation system prompt and history so the compactor
    // request can share the provider's prompt cache prefix. Compact-specific
    // instructions are appended as the final user message.
    let mut summary_messages = messages;
    summary_messages.push(Arc::new(Message::user(SUMMARY_PROMPT)));

    let summary_config = crate::provider::resolve_request_config(
        &summary_messages,
        tools,
        &ModelConfig {
            max_tokens: Some(summary_max_tokens),
            thinking: crate::provider::ThinkingConfig::default(),
            ..model_config.clone()
        },
    )
    .map_err(CompactionError::from)?;

    // Spawn provider request in a separate task to allow cancellation
    let summary_messages_clone = summary_messages;
    let tools_clone = tools.to_vec();
    let summary_config_clone = summary_config;
    let provider_clone = Arc::clone(&provider);
    let stream_task = tokio::spawn(async move {
        provider_clone
            .stream(&summary_messages_clone, &tools_clone, &summary_config_clone)
            .await
    });
    let abort_handle = stream_task.abort_handle();

    // Call API with cancellation support
    let mut stream = tokio::select! {
        biased;
        () = async {
            if let Some(ref t) = cancel_token {
                t.cancelled().await
            } else {
                std::future::pending().await
            }
        } => {
            abort_handle.abort();
            return Err(CompactionError::Cancelled);
        }
        result = stream_task => match result {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return Err(e.into()),
            Err(e) if e.is_cancelled() => return Err(CompactionError::Cancelled),
            Err(e) => return Err(CompactionError::Api(format!("Summary stream task panicked: {e}"))),
        }
    };

    // Collect response with cancellation check
    let mut summary = String::new();
    let mut token_usage = crate::provider::TokenUsage::default();
    let mut finish_reason = None;

    loop {
        let item = tokio::select! {
            biased;
            () = async {
                if let Some(ref t) = cancel_token {
                    t.cancelled().await
                } else {
                    std::future::pending().await
                }
            } => {
                return Err(CompactionError::Cancelled);
            }
            item = stream.try_next() => match item {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(e) => return Err(e.into()),
            }
        };

        match item {
            ModelStreamItem::Chunk(crate::event::ContentChunk::Text(text)) => {
                summary.push_str(&text);
            }
            ModelStreamItem::TokenUsage(usage) => {
                token_usage = usage;
            }
            ModelStreamItem::ResponseMeta {
                finish_reason: reason,
                ..
            } => {
                finish_reason = reason;
            }
            ModelStreamItem::ToolCall(_) | ModelStreamItem::ToolCallDelta { .. } => {
                return Err(CompactionError::Api(
                    "Summary generation attempted to call a tool".to_string(),
                ));
            }
            ModelStreamItem::Complete => break,
            _ => {}
        }
    }
    if finish_reason != Some(FinishReason::Stop) {
        return Err(CompactionError::Api(format!(
            "Summary generation did not finish normally: {finish_reason:?}"
        )));
    }
    if summary.trim().is_empty() {
        return Err(CompactionError::Api(
            "Summary generation returned an empty summary".to_string(),
        ));
    }
    let summary = parse_summary_xml(&summary)?;
    Ok((summary, token_usage))
}

/// Build the user-facing continuation message stored after compaction.
fn build_continuation_summary(summary: &str) -> String {
    format!(
        "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n{summary}\n\nContinue the conversation from where it left off without asking the user to repeat information already included here. Resume the latest unfinished task directly. Do not acknowledge this summary, do not recap the conversation, and do not preface the response with phrases such as \"I'll continue\"."
    )
}

/// Extract the durable summary from the model's XML response and discard the
/// private drafting block. When the model omits the optional XML wrappers, keep
/// the text rather than losing a potentially useful summary.
fn parse_summary_xml(raw: &str) -> Result<String, CompactionError> {
    if let Some(summary_start) = raw.find("<summary>") {
        let content_start = summary_start + "<summary>".len();
        let end = raw[content_start..]
            .find("</summary>")
            .map(|offset| content_start + offset)
            .ok_or_else(|| {
                CompactionError::Api(
                    "Summary generation returned an unclosed <summary> block".to_string(),
                )
            })?;
        let summary = raw[content_start..end].trim();
        if summary.is_empty() {
            return Err(CompactionError::Api(
                "Summary generation returned an empty <summary> block".to_string(),
            ));
        }
        return Ok(format!("Summary:\n{summary}"));
    }

    if raw.contains("<analysis>") || raw.contains("</analysis>") || raw.contains("</summary>") {
        return Err(CompactionError::Api(
            "Summary generation returned malformed XML".to_string(),
        ));
    }

    let summary = raw.trim();
    if summary.is_empty() {
        return Err(CompactionError::Api(
            "Summary generation returned an empty summary".to_string(),
        ));
    }
    Ok(summary.to_string())
}

#[cfg(test)]
mod tests;
