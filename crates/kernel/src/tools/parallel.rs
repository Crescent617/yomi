use crate::event::ToolEvent;
use crate::tools::{Tool, ToolExecCtx, ToolRegistry};
use crate::types::{AgentId, ContentBlock, Message, Role, ToolCall, ToolOutput};
use crate::utils::strs;
use std::sync::Arc;
use tokio::task::JoinSet;

use tokio_util::sync::CancellationToken;

const MAX_OUTPUT_LENGTH: usize = 40_000;
const TRUNCATION_MESSAGE: &str = "\n\n[Output truncated due to length.]";

/// Tool execution result
pub struct ToolExecutionResult {
    pub tool_call_id: String,
    pub message: Message,
    pub event: ToolEvent,
}

/// Truncate output if it exceeds max length (UTF-8 safe)
fn truncate_output(output: &str) -> String {
    strs::truncate_with_suffix(output, MAX_OUTPUT_LENGTH, TRUNCATION_MESSAGE)
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
) -> Vec<ToolExecutionResult> {
    let tool_count = tool_calls.len();
    tracing::info!(
        "Executing {} tool(s) in parallel for agent {}",
        tool_count,
        agent_id
    );

    let mut join_set = JoinSet::new();

    for call in tool_calls {
        tracing::debug!("Looking up tool: '{}'", call.name);
    }

    for call in tool_calls {
        let agent_id = agent_id.clone();
        // Clone necessary fields to avoid holding references
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

        // Clone parent_messages and cancel_token for the async block
        let parent_messages_for_task = parent_messages.map(|msgs| msgs.to_vec());
        let cancel_token_for_task = cancel_token.cloned();

        join_set.spawn(async move {
            let start = std::time::Instant::now();
            let result = match tool_opt {
                Some(tool) => {
                    let ctx = ToolExecCtx::with_parent_ctx(
                        &call_id,
                        parent_messages_for_task.as_deref(),
                        cancel_token_for_task,
                    );
                    execute_single_tool_with_ctx(tool, arguments, ctx).await
                }
                None => ToolOutput {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Unknown tool: {call_name}"),
                },
            };
            let elapsed = start.elapsed().as_millis() as u64;
            let success = result.success();

            // Truncate output if too long
            let stdout = truncate_output(&result.stdout);
            let stderr = truncate_output(&result.stderr);

            let (event, message) = if success {
                let output = stdout;
                (
                    ToolEvent::Output {
                        agent_id: agent_id.clone(),
                        tool_id: call_id.clone(),
                        output: output.clone(),
                        elapsed_ms: elapsed,
                    },
                    Message {
                        role: Role::Tool,
                        content: vec![ContentBlock::Text { text: output }],
                        tool_calls: None,
                        tool_call_id: Some(call_id.clone()),
                        created_at: chrono::Utc::now(),
                        token_usage: None,
                    },
                )
            } else {
                let error = format!("Exit code: {}\n{}\n{}", result.exit_code, stdout, stderr);
                (
                    ToolEvent::Error {
                        agent_id: agent_id.clone(),
                        tool_id: call_id.clone(),
                        error: error.clone(),
                        elapsed_ms: elapsed,
                    },
                    Message {
                        role: Role::Tool,
                        content: vec![ContentBlock::Text { text: error }],
                        tool_calls: None,
                        tool_call_id: Some(call_id.clone()),
                        created_at: chrono::Utc::now(),
                        token_usage: None,
                    },
                )
            };

            ToolExecutionResult {
                tool_call_id: call_id,
                message,
                event,
            }
        });
    }

    let mut results = Vec::new();

    // If cancel_token is provided, use select! to wait for either completion or cancellation
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
                            if let ToolEvent::Output { elapsed_ms, .. } = &r.event {
                                tracing::debug!(
                                    "Tool {} completed successfully in {}ms",
                                    r.tool_call_id,
                                    elapsed_ms
                                );
                            } else if let ToolEvent::Error { error, elapsed_ms, .. } = &r.event {
                                tracing::warn!(
                                    "Tool {} failed in {}ms: {}",
                                    r.tool_call_id,
                                    elapsed_ms,
                                    error
                                );
                            }
                            results.push(r);
                        }
                        Some(Err(e)) => {
                            tracing::warn!("Tool task panicked: {}", e);
                        }
                        None => break, // All tasks completed
                    }
                }
            }
        }
    } else {
        // Original behavior without cancellation
        while let Some(Ok(result)) = join_set.join_next().await {
            if let ToolEvent::Output { elapsed_ms, .. } = &result.event {
                tracing::debug!(
                    "Tool {} completed successfully in {}ms",
                    result.tool_call_id,
                    elapsed_ms
                );
            } else if let ToolEvent::Error {
                error, elapsed_ms, ..
            } = &result.event
            {
                tracing::warn!(
                    "Tool {} failed in {}ms: {}",
                    result.tool_call_id,
                    elapsed_ms,
                    error
                );
            }
            results.push(result);
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
        Err(e) => ToolOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("Tool execution error: {e}"),
        },
    }
}
