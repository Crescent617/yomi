//! Shared stream collection logic for Agent
//!
//! This module provides common functionality for collecting model stream output,
//! handling thinking content, text chunks, tool calls, and token usage.

use crate::event::ContentChunk;
use crate::types::{ContentBlock, FinishReason, ToolCall};

/// Interval (estimated tokens) at which streaming tool-call arguments emit a
/// progress summary. Large arguments (e.g. big file writes) otherwise stream
/// for a long time without leaving any trace in the logs.
const TOOL_CALL_SUMMARY_TOKEN_INTERVAL: usize = 4_096;

/// Max chars kept for the head/tail argument snippets in the summary.
const TOOL_CALL_SUMMARY_SNIPPET_CHARS: usize = 80;

/// Accumulator for one streaming tool call's argument deltas.
#[derive(Debug)]
struct ToolCallDeltaTracker {
    id: String,
    name: String,
    /// Accumulated argument bytes; estimated tokens = bytes / 4.
    bytes: usize,
    /// Head snippet of the arguments (identifies what the call targets).
    head: String,
    /// Tail snippet of the arguments (shows what is being written now).
    tail: String,
    /// Next estimated-token threshold that triggers a summary.
    next_threshold: usize,
}

impl ToolCallDeltaTracker {
    fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            bytes: 0,
            head: String::new(),
            tail: String::new(),
            next_threshold: TOOL_CALL_SUMMARY_TOKEN_INTERVAL,
        }
    }
}

/// Result of collecting stream output
#[derive(Debug, Default)]
pub struct StreamCollectionResult {
    pub content_blocks: Vec<ContentBlock>,
    pub tool_calls: Vec<ToolCall>,
    /// Token usage
    pub token_usage: Option<crate::provider::TokenUsage>,
    /// API response ID (e.g., "chatcmpl-xxx" or "`msg_xxx`")
    pub response_id: Option<String>,
    /// Finish/stop reason (normalized across providers)
    pub finish_reason: Option<FinishReason>,
}

/// Internal state for stream collection
#[derive(Default)]
pub struct StreamCollectorState {
    current_text: String,
    current_thinking: String,
    thinking_signature: Option<String>,
    has_redacted_thinking: bool,
    pending_tool_calls: Vec<ToolCall>,
    /// Token usage
    token_usage: Option<crate::provider::TokenUsage>,
    /// API response ID
    response_id: Option<String>,
    /// Finish/stop reason
    finish_reason: Option<FinishReason>,
    /// Accumulator for the tool call currently streaming. Deltas for one
    /// call arrive contiguously, so a new tool call id replaces this state.
    tool_call_delta: Option<ToolCallDeltaTracker>,
}

impl StreamCollectorState {
    /// Handle a content chunk, updating internal state
    pub(crate) fn handle_chunk(&mut self, chunk: &ContentChunk) {
        match chunk {
            ContentChunk::Text(text) => {
                self.current_text.push_str(text);
            }
            ContentChunk::Thinking {
                thinking,
                signature,
            } => {
                self.current_thinking.push_str(thinking);
                if signature.is_some() {
                    self.thinking_signature.clone_from(signature);
                }
            }
            ContentChunk::RedactedThinking => {
                self.has_redacted_thinking = true;
            }
        }
    }

    pub(crate) fn handle_tool_call(&mut self, request: crate::provider::ToolCallRequest) {
        self.pending_tool_calls.push(ToolCall {
            id: request.id,
            name: request.name,
            arguments: request.arguments,
        });
    }

    /// Accumulate a streaming tool-call argument delta. Returns a summary
    /// string each time the accumulated size crosses another
    /// [`TOOL_CALL_SUMMARY_TOKEN_INTERVAL`] estimated-token boundary.
    pub(crate) fn handle_tool_call_delta(
        &mut self,
        id: &str,
        name: &str,
        delta: &str,
    ) -> Option<String> {
        let tracker = self
            .tool_call_delta
            .get_or_insert_with(|| ToolCallDeltaTracker::new(id, name));
        if tracker.id != id {
            *tracker = ToolCallDeltaTracker::new(id, name);
        }
        // The name chunk may arrive after the first args delta (providers
        // emit an empty name until then); fill it in once known.
        if tracker.name.is_empty() && !name.is_empty() {
            name.clone_into(&mut tracker.name);
        }
        tracker.bytes += delta.len();

        // Grow the head snippet up to the cap (first deltas may be empty).
        let head_len = tracker.head.chars().count();
        if head_len < TOOL_CALL_SUMMARY_SNIPPET_CHARS {
            tracker.head.extend(
                delta
                    .chars()
                    .take(TOOL_CALL_SUMMARY_SNIPPET_CHARS - head_len),
            );
        }

        // Keep only the tail of the stream, trimmed at a char boundary.
        tracker.tail.push_str(delta);
        let tail_len = tracker.tail.chars().count();
        if tail_len > TOOL_CALL_SUMMARY_SNIPPET_CHARS {
            let skip = tail_len - TOOL_CALL_SUMMARY_SNIPPET_CHARS;
            if let Some((idx, _)) = tracker.tail.char_indices().nth(skip) {
                tracker.tail.drain(..idx);
            }
        }

        // 4 bytes ≈ 1 token, same heuristic as `utils::tokens::estimate_tokens`.
        let tokens = tracker.bytes / 4;
        if tokens < tracker.next_threshold {
            return None;
        }
        // Advance past the current token count so one huge delta logs once.
        tracker.next_threshold = tokens / TOOL_CALL_SUMMARY_TOKEN_INTERVAL
            * TOOL_CALL_SUMMARY_TOKEN_INTERVAL
            + TOOL_CALL_SUMMARY_TOKEN_INTERVAL;
        // Debug-format the snippets to keep the summary on a single line.
        Some(format!(
            "tool call `{}` ({id}) streaming: {} tokens accumulated, args head: {:?}, tail: {:?}",
            tracker.name,
            crate::utils::tokens::format_estimated_tokens(tokens),
            tracker.head,
            tracker.tail
        ))
    }

    pub(crate) fn handle_token_usage(&mut self, usage: crate::provider::TokenUsage) {
        self.token_usage = Some(usage);
    }

    pub(crate) fn handle_response_meta(
        &mut self,
        response_id: Option<String>,
        finish_reason: Option<FinishReason>,
    ) {
        self.response_id = response_id;
        self.finish_reason = finish_reason;
    }

    /// Build content blocks, tool calls, and token usage from collected state
    pub(crate) fn build_result(self) -> StreamCollectionResult {
        let mut content_blocks = Vec::new();

        // Add redacted thinking if present (before regular thinking)
        if self.has_redacted_thinking {
            content_blocks.push(ContentBlock::RedactedThinking {
                data: String::new(),
            });
        }

        // Add thinking content first (if present)
        if !self.current_thinking.is_empty() {
            content_blocks.push(ContentBlock::Thinking {
                thinking: self.current_thinking,
                signature: self.thinking_signature,
            });
        }

        // Add text content
        if !self.current_text.is_empty() {
            content_blocks.push(ContentBlock::Text {
                text: self.current_text,
            });
        }

        StreamCollectionResult {
            content_blocks,
            tool_calls: self.pending_tool_calls,
            token_usage: self.token_usage,
            response_id: self.response_id,
            finish_reason: self.finish_reason,
        }
    }
}

#[cfg(test)]
#[path = "stream_collector_test.rs"]
mod tests;
