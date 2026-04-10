use crate::event::ToolEvent;
use crate::tool::{Tool, ToolRegistry};
use crate::types::{AgentId, ContentBlock, Message, Role, ToolCall, ToolOutput};
use std::sync::Arc;
use tokio::task::JoinSet;

/// Max output length before truncation (10KB)
const MAX_OUTPUT_LENGTH: usize = 10_000;
const TRUNCATION_MESSAGE: &str = "\n\n[Output truncated due to length. Use file tools or pagination to view full output.]";

/// Tool execution result
pub struct ToolExecutionResult {
    pub tool_call_id: String,
    pub message: Message,
    pub event: ToolEvent,
}

/// Truncate output if it exceeds max length
fn truncate_output(output: String) -> String {
    if output.len() > MAX_OUTPUT_LENGTH {
        let truncate_at = MAX_OUTPUT_LENGTH - TRUNCATION_MESSAGE.len();
        format!("{}{}", &output[..truncate_at], TRUNCATION_MESSAGE)
    } else {
        output
    }
}

/// Execute multiple tool calls in parallel
pub async fn execute_tools_parallel(
    agent_id: &AgentId,
    tool_calls: &[ToolCall],
    tool_registry: &ToolRegistry,
) -> Vec<ToolExecutionResult> {
    let tool_count = tool_calls.len();
    tracing::info!(
        "Executing {} tool(s) in parallel for agent {}",
        tool_count,
        agent_id.0
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

        join_set.spawn(async move {
            let start = std::time::Instant::now();
            let result = match tool_opt {
                Some(tool) => execute_single_tool(tool, arguments).await,
                None => ToolOutput {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Unknown tool: {call_name}"),
                },
            };
            let elapsed = start.elapsed().as_millis() as u64;
            let success = result.success();

            // Truncate output if too long
            let stdout = truncate_output(result.stdout);
            let stderr = truncate_output(result.stderr);

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

async fn execute_single_tool(tool: Arc<dyn Tool>, arguments: serde_json::Value) -> ToolOutput {
    match tool.execute(arguments).await {
        Ok(output) => output,
        Err(e) => ToolOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("Tool execution error: {e}"),
        },
    }
}
