//! Shared stream collection logic for Agent and SimpleAgent
//!
//! This module provides common functionality for collecting model stream output,
//! handling thinking content, text chunks, tool calls, and token usage.

use crate::event::ContentChunk;
use crate::types::{ContentBlock, ToolCall};

/// Result of collecting stream output
#[derive(Debug, Default)]
pub struct StreamCollectionResult {
    pub content_blocks: Vec<ContentBlock>,
    pub tool_calls: Vec<ToolCall>,
    pub token_usage: Option<(u32, u32)>,
}

/// Internal state for stream collection
#[derive(Default)]
pub struct StreamCollectorState {
    current_text: String,
    current_thinking: String,
    thinking_signature: Option<String>,
    has_redacted_thinking: bool,
    pending_tool_calls: Vec<ToolCall>,
    token_usage: Option<(u32, u32)>,
}

impl StreamCollectorState {
    /// Handle a content chunk, updating internal state
    pub(crate) fn handle_chunk(&mut self, chunk: &ContentChunk) {
        match chunk {
            ContentChunk::Text(text) => {
                self.current_text.push_str(text);
            }
            ContentChunk::Thinking { thinking, signature } => {
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

    pub(crate) fn handle_tool_call(&mut self, request: crate::providers::ToolCallRequest) {
        self.pending_tool_calls.push(ToolCall {
            id: request.id,
            name: request.name,
            arguments: request.arguments,
        });
    }

    pub(crate) fn handle_token_usage(&mut self, prompt_tokens: u32, completion_tokens: u32) {
        self.token_usage = Some((prompt_tokens, completion_tokens));
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
        }
    }
}

