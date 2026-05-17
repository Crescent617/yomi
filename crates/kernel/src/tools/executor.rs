use crate::event::ToolEvent;
use crate::tools::helper::truncate::{truncate_output, TRUNCATION_MESSAGE};
use crate::tools::{Tool, ToolExecCtx, ToolRegistry, READ_TOOL_NAME};
use crate::types::{AgentId, ContentBlock, Message, MessageId, Role, ToolCall, ToolOutput};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinSet;

use tokio_util::sync::CancellationToken;

/// Maximum tool output length (20 KB)
const MAX_TOOL_OUTPUT_LENGTH: usize = 20_000;

/// Tool execution result
pub struct ToolExecutionResult {
    pub tool_call_id: String,
    pub message_id: MessageId,
    pub message: Message,
    pub event: ToolEvent,
}

/// Check if a tool handles its own truncation
fn tool_handles_truncation(tool_name: &str) -> bool {
    tool_name == READ_TOOL_NAME
}

/// Truncate and convert `ToolOutputBlock` to `ContentBlock`
fn truncate_and_convert_blocks(
    blocks: &[crate::types::ToolOutputBlock],
    tool_name: &str,
) -> Vec<crate::types::ToolOutputBlock> {
    // Skip truncation for tools that handle it themselves
    let should_truncate = !tool_handles_truncation(tool_name);

    blocks
        .iter()
        .map(|block| match block {
            crate::types::ToolOutputBlock::Text { text } => crate::types::ToolOutputBlock::Text {
                text: if should_truncate {
                    truncate_output(text, MAX_TOOL_OUTPUT_LENGTH, TRUNCATION_MESSAGE)
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

/// Convert `ToolOutputBlock` to `ContentBlock`
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

/// Build a tool result (success or error) from tool output.
fn build_tool_result(
    agent_id: &AgentId,
    call_id: &str,
    tool_name: &str,
    output: &ToolOutput,
    elapsed_ms: u64,
    message_id: MessageId,
) -> (ToolEvent, Message) {
    let (blocks, content, is_error) = if output.success() {
        let truncated = truncate_and_convert_blocks(&output.contents, tool_name);
        let content = to_content_blocks(&truncated);
        (truncated, content, false)
    } else {
        let text = format!("Error: {}", output.error_text());
        let blocks = vec![crate::types::ToolOutputBlock::Text { text: text.clone() }];
        let content = vec![ContentBlock::Text { text }];
        (blocks, content, true)
    };

    let event = ToolEvent::End {
        agent_id: agent_id.clone(),
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

/// Extract text from content blocks for logging
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

/// Log result and push to results vector
fn log_and_push_result(results: &mut Vec<ToolExecutionResult>, result: ToolExecutionResult) {
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
            tracing::debug!(
                "Tool {} completed successfully in {}ms",
                result.tool_call_id,
                elapsed_ms
            );
        }
    }
    results.push(result);
}

/// Execute multiple tool calls in parallel with optional cancellation support
///
/// Accepts tokio native `CancellationToken` for runtime cancellation control.
/// The `cancel_token` should be created from Agent's custom `CancelToken` at the
/// start of each request.
///
/// `message_ids` maps each `tool_call_id` to a pre-generated `MessageId` for the
/// resulting tool result message. This allows `ToolEvent::Start` and `ToolEvent::End`
/// to carry a consistent message identifier.
///
/// `checkpoint_manager` is used to immediately backup files when tools call `track_edit`,
/// ensuring checkpoints capture the state BEFORE modification.
///
/// Returns both the execution results and any files that were tracked for checkpointing.
#[allow(clippy::too_many_arguments, clippy::implicit_hasher)]
pub async fn execute_tools_parallel(
    agent_id: &AgentId,
    tool_calls: &[ToolCall],
    tool_registry: &ToolRegistry,
    cancel_token: Option<&CancellationToken>,
    parent_messages: Option<&[Arc<Message>]>,
    working_dir: &std::path::Path,
    session_id: &str,
    message_ids: &HashMap<String, MessageId>,
    turn: Option<Arc<crate::agent::Turn>>,
) -> Vec<ToolExecutionResult> {
    let tool_count = tool_calls.len();
    tracing::info!(
        "Executing {} tool(s) in parallel for agent {}",
        tool_count,
        agent_id
    );

    let mut join_set = JoinSet::new();

    for call in tool_calls {
        let agent_id = agent_id.clone();
        let call_id = call.id.clone();
        let call_name = call.name.clone();
        let arguments = call.arguments.clone();
        let tool_opt = tool_registry.get(&call_name);
        let session_id = session_id.to_string();
        let message_id = message_ids[&call_id].clone();

        if tool_opt.is_none() {
            tracing::error!(
                "Tool '{}' not found in registry. Available tools: {:?}",
                call_name,
                tool_registry.list()
            );
        }

        let parent_messages_for_task = parent_messages.map(|msgs| msgs.to_vec());
        let cancel_token_for_task = cancel_token.cloned();
        let working_dir = working_dir.to_path_buf();
        let turn_for_task = turn.clone();

        join_set.spawn(async move {
            let start = std::time::Instant::now();
            let mut ctx = ToolExecCtx::with_parent_ctx(
                &call_id,
                parent_messages_for_task.as_deref(),
                cancel_token_for_task,
                &working_dir,
                session_id,
                message_id.clone(),
            );
            ctx.turn = turn_for_task;
            let result = match tool_opt {
                Some(tool) => execute_single_tool_with_ctx(tool, arguments, ctx).await,
                None => ToolOutput::error(format!("Unknown tool: {call_name}")),
            };
            let elapsed = start.elapsed().as_millis() as u64;

            let (event, message) = build_tool_result(
                &agent_id,
                &call_id,
                &call_name,
                &result,
                elapsed,
                message_id.clone(),
            );

            ToolExecutionResult {
                tool_call_id: call_id,
                message_id,
                message,
                event,
            }
        });
    }

    let mut results = Vec::new();

    if let Some(token) = cancel_token {
        loop {
            tokio::select! {
                biased;
                () = token.cancelled() => {
                    tracing::info!("Tool execution cancelled, aborting {} remaining tasks", join_set.len());
                    join_set.abort_all();
                    break;
                }
                result = join_set.join_next() => {
                    match result {
                        Some(Ok(r)) => {
                            log_and_push_result(&mut results, r);
                        }
                        Some(Err(e)) => tracing::warn!("Tool task panicked: {}", e),
                        None => break,
                    }
                }
            }
        }
    } else {
        while let Some(Ok(result)) = join_set.join_next().await {
            log_and_push_result(&mut results, result);
        }
    }

    let success_count = results
        .iter()
        .filter(|r| {
            matches!(
                r.event,
                ToolEvent::End {
                    is_error: false,
                    ..
                }
            )
        })
        .count();
    tracing::info!(
        "Tool execution completed: {}/{} succeeded",
        success_count,
        tool_count
    );

    results
}

async fn execute_single_tool_with_ctx(
    tool: Arc<dyn Tool>,
    arguments: serde_json::Value,
    ctx: ToolExecCtx<'_>,
) -> ToolOutput {
    match tool.exec(arguments, ctx).await {
        Ok(output) => output,
        Err(e) => ToolOutput::error(format!("Tool execution error: {e}")),
    }
}
