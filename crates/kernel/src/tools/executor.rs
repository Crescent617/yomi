use crate::event::ToolEvent;
use crate::tools::helper::truncate::{truncate_output, TRUNCATION_MESSAGE};
use crate::tools::{Tool, ToolExecCtx, READ_TOOL_NAME, SHELL_TOOL_NAME};
use crate::types::{ContentBlock, Message, MessageId, Role, ToolOutput};
use std::sync::Arc;

/// The result of executing a single tool call, ready to emit and persist.
pub struct ToolExecutionResult {
    pub tool_call_id: String,
    pub message_id: MessageId,
    pub message: Message,
    pub event: ToolEvent,
}

/// Check if a tool handles its own truncation.
fn tool_handles_truncation(tool_name: &str) -> bool {
    tool_name == READ_TOOL_NAME || tool_name == SHELL_TOOL_NAME
}

/// Truncate tool output blocks, skipping tools that manage truncation themselves.
fn truncate_blocks(
    blocks: &[crate::types::ToolOutputBlock],
    tool_name: &str,
    max_len: usize,
) -> Vec<crate::types::ToolOutputBlock> {
    let should_truncate = !tool_handles_truncation(tool_name);
    blocks
        .iter()
        .map(|block| match block {
            crate::types::ToolOutputBlock::Text { text } => crate::types::ToolOutputBlock::Text {
                text: if should_truncate {
                    truncate_output(text, max_len, TRUNCATION_MESSAGE)
                } else {
                    text.clone()
                },
            },
            crate::types::ToolOutputBlock::Image { url, mime_type } => {
                crate::types::ToolOutputBlock::Image {
                    url: url.clone(),
                    mime_type: mime_type.clone(),
                }
            }
        })
        .collect()
}

/// Convert `ToolOutputBlock`s to `ContentBlock`s for the message.
/// Image blocks are NOT normalized here — this builder is sync and
/// recompression needs the blocking pool; the real-execution call site
/// in `tool_exec` normalizes via `utils::image::normalize_image_blocks`
/// (error outputs carry no images and skip it).
fn to_content_blocks(blocks: &[crate::types::ToolOutputBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|block| match block {
            crate::types::ToolOutputBlock::Text { text } => {
                ContentBlock::Text { text: text.clone() }
            }
            crate::types::ToolOutputBlock::Image { url, mime_type: _ } => ContentBlock::ImageUrl {
                image_url: crate::types::ImageUrl {
                    url: url.clone(),
                    detail: None,
                },
            },
        })
        .collect()
}

/// Build a `ToolExecutionResult` from a raw `ToolOutput`.
/// Handles both success and error cases, applying truncation where appropriate.
pub fn build_tool_result(
    call_id: &str,
    tool_name: &str,
    output: &ToolOutput,
    elapsed_ms: u64,
    message_id: MessageId,
    max_tool_output_length: usize,
) -> (ToolEvent, Message) {
    let (blocks, content, is_error) = if output.success() {
        let truncated = truncate_blocks(&output.contents, tool_name, max_tool_output_length);
        let content = to_content_blocks(&truncated);
        (truncated, content, false)
    } else {
        let text = format!("Error: {}", output.error_text());
        let blocks = vec![crate::types::ToolOutputBlock::Text { text: text.clone() }];
        let content = vec![ContentBlock::Text { text }];
        (blocks, content, true)
    };

    let event = ToolEvent::End {
        message_id: message_id.clone(),
        tool_id: call_id.to_string(),
        tool_name: tool_name.to_string(),
        content_blocks: blocks,
        elapsed_ms,
        is_error,
    };

    let message = Message {
        id: message_id,
        role: Role::Tool,
        content,
        tool_call_id: Some(call_id.to_string()),
        ..Default::default()
    };

    (event, message)
}

/// Log a completed tool result.
pub fn log_tool_result(result: &ToolExecutionResult) {
    if let ToolEvent::End {
        elapsed_ms,
        is_error,
        content_blocks,
        ..
    } = &result.event
    {
        if *is_error {
            let text = content_blocks_to_text(content_blocks);
            tracing::warn!(
                "Tool {} failed in {}ms: {}",
                result.tool_call_id,
                elapsed_ms,
                text
            );
        } else {
            tracing::debug!("Tool {} completed in {}ms", result.tool_call_id, elapsed_ms);
        }
    }
}

/// Extract plain text from tool output blocks (for logging).
fn content_blocks_to_text(blocks: &[crate::types::ToolOutputBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            crate::types::ToolOutputBlock::Text { text } => Some(text.as_str()),
            crate::types::ToolOutputBlock::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .concat()
}

/// Execute a single tool and return its raw output.
/// This is the pure execution primitive — no emit or persistence.
pub async fn execute_single_tool(
    tool: Arc<dyn Tool>,
    arguments: serde_json::Value,
    ctx: ToolExecCtx<'_>,
) -> ToolOutput {
    match tool.exec(arguments, ctx).await {
        Ok(output) => output,
        Err(e) => ToolOutput::error(format!("Tool execution error: {e}")),
    }
}

#[cfg(test)]
#[path = "executor_test.rs"]
mod tests;
