use crate::event::ToolEvent;
use crate::tools::helper::truncate::{truncate_output, TRUNCATION_MESSAGE};
use crate::tools::{Tool, ToolExecCtx, ToolRegistry, READ_TOOL_NAME};
use crate::types::{AgentId, ContentBlock, Message, Role, ToolCall, ToolOutput};
use std::sync::Arc;
use tokio::task::JoinSet;

use tokio_util::sync::CancellationToken;

/// Maximum tool output length (20 KB)
const MAX_TOOL_OUTPUT_LENGTH: usize = 20_000;

/// Tool execution result
pub struct ToolExecutionResult {
    pub tool_call_id: String,
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

/// Build success result from tool output
fn build_success_result(
    agent_id: &AgentId,
    call_id: &str,
    tool_name: &str,
    result: &ToolOutput,
    elapsed_ms: u64,
) -> (ToolEvent, Message) {
    let truncated = truncate_and_convert_blocks(&result.contents, tool_name);
    let content_blocks = to_content_blocks(&truncated);
    // Skip truncation for tools that handle it themselves
    let output = if tool_handles_truncation(tool_name) {
        result.text_content()
    } else {
        truncate_output(
            &result.text_content(),
            MAX_TOOL_OUTPUT_LENGTH,
            TRUNCATION_MESSAGE,
        )
    };

    let event = ToolEvent::Output {
        agent_id: agent_id.clone(),
        tool_id: call_id.to_string(),
        output,
        content_blocks: truncated,
        elapsed_ms,
    };

    let message = Message {
        role: Role::Tool,
        content: content_blocks,
        tool_call_id: Some(call_id.to_string()),
        ..Default::default()
    };

    (event, message)
}

/// Build error result from tool output
fn build_error_result(
    agent_id: &AgentId,
    call_id: &str,
    result: &ToolOutput,
    elapsed_ms: u64,
) -> (ToolEvent, Message) {
    let error = format!("Error: {}", result.error_text());

    let event = ToolEvent::Error {
        agent_id: agent_id.clone(),
        tool_id: call_id.to_string(),
        error: error.clone(),
        content_blocks: Vec::new(),
        elapsed_ms,
    };

    let message = Message {
        role: Role::Tool,
        content: vec![ContentBlock::Text { text: error }],
        tool_call_id: Some(call_id.to_string()),
        ..Default::default()
    };

    (event, message)
}

/// Log result and push to results vector
fn log_and_push_result(results: &mut Vec<ToolExecutionResult>, result: ToolExecutionResult) {
    match &result.event {
        ToolEvent::Output { elapsed_ms, .. } => {
            tracing::debug!(
                "Tool {} completed successfully in {}ms",
                result.tool_call_id,
                elapsed_ms
            );
        }
        ToolEvent::Error {
            error, elapsed_ms, ..
        } => {
            tracing::warn!(
                "Tool {} failed in {}ms: {}",
                result.tool_call_id,
                elapsed_ms,
                error
            );
        }
        _ => {}
    }
    results.push(result);
}

/// Execute multiple tool calls in parallel with optional cancellation support
///
/// Accepts tokio native `CancellationToken` for runtime cancellation control.
/// The `cancel_token` should be created from Agent's custom `CancelToken` at the
/// start of each request.
pub async fn execute_tools_parallel(
    agent_id: &AgentId,
    tool_calls: &[ToolCall],
    tool_registry: &ToolRegistry,
    cancel_token: Option<&CancellationToken>,
    parent_messages: Option<&[Arc<Message>]>,
    working_dir: &std::path::Path,
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

        join_set.spawn(async move {
            let start = std::time::Instant::now();
            let result = match tool_opt {
                Some(tool) => {
                    let ctx = ToolExecCtx::with_parent_ctx(
                        &call_id,
                        parent_messages_for_task.as_deref(),
                        cancel_token_for_task,
                        &working_dir,
                    );
                    execute_single_tool_with_ctx(tool, arguments, ctx).await
                }
                None => ToolOutput::error(format!("Unknown tool: {call_name}")),
            };
            let elapsed = start.elapsed().as_millis() as u64;

            let (event, message) = if result.success() {
                build_success_result(&agent_id, &call_id, &call_name, &result, elapsed)
            } else {
                build_error_result(&agent_id, &call_id, &result, elapsed)
            };

            ToolExecutionResult {
                tool_call_id: call_id,
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
                        Some(Ok(r)) => log_and_push_result(&mut results, r),
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
        .filter(|r| matches!(r.event, ToolEvent::Output { .. }))
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
