use crate::event::ToolEvent;
use crate::tool::{Tool, ToolRegistry, ToolSandbox};
use crate::types::{AgentId, ContentBlock, Message, Role, ToolCall, ToolOutput};
use std::sync::Arc;
use tokio::task::JoinSet;

/// 工具执行结果
pub struct ToolExecutionResult {
    pub tool_call_id: String,
    pub message: Message,
    pub event: ToolEvent,
}

/// 并行执行多个工具调用
pub async fn execute_tools_parallel(
    agent_id: &AgentId,
    tool_calls: Vec<ToolCall>,
    tool_registry: &ToolRegistry,
    _sandbox: &ToolSandbox,
    timeout: std::time::Duration,
) -> Vec<ToolExecutionResult> {
    let tool_count = tool_calls.len();
    tracing::info!(
        "Executing {} tool(s) in parallel for agent {}",
        tool_count,
        agent_id.0
    );

    let mut join_set = JoinSet::new();

    for call in tool_calls {
        let agent_id = agent_id.clone();
        let tool_opt = tool_registry.get(&call.name);

        join_set.spawn(async move {
            let result = match tool_opt {
                Some(tool) => execute_single_tool(tool, call.clone(), timeout).await,
                None => ToolOutput {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Unknown tool: {}", call.name),
                },
            };

            let (event, message) = if result.success() {
                let output = result.stdout;
                (
                    ToolEvent::Output {
                        agent_id: agent_id.clone(),
                        tool_id: call.id.clone(),
                        output: output.clone(),
                    },
                    Message {
                        role: Role::Tool,
                        content: vec![ContentBlock::Text { text: output }],
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        created_at: chrono::Utc::now(),
                    },
                )
            } else {
                let error = format!(
                    "Exit code: {}\n{}\n{}",
                    result.exit_code, result.stdout, result.stderr
                );
                (
                    ToolEvent::Error {
                        agent_id: agent_id.clone(),
                        tool_id: call.id.clone(),
                        error: error.clone(),
                    },
                    Message {
                        role: Role::Tool,
                        content: vec![ContentBlock::Text { text: error }],
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        created_at: chrono::Utc::now(),
                    },
                )
            };

            ToolExecutionResult {
                tool_call_id: call.id,
                message,
                event,
            }
        });
    }

    let mut results = Vec::new();
    while let Some(Ok(result)) = join_set.join_next().await {
        if let ToolEvent::Output { .. } = &result.event {
            tracing::debug!("Tool {} completed successfully", result.tool_call_id);
        } else if let ToolEvent::Error { error, .. } = &result.event {
            tracing::warn!("Tool {} failed: {}", result.tool_call_id, error);
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

async fn execute_single_tool(
    tool: Arc<dyn Tool>,
    call: ToolCall,
    timeout: std::time::Duration,
) -> ToolOutput {
    match tokio::time::timeout(timeout, tool.execute(call.arguments)).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => ToolOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("Tool execution error: {e}"),
        },
        Err(_) => ToolOutput {
            exit_code: 124,
            stdout: String::new(),
            stderr: format!("Tool execution timed out after {timeout:?}"),
        },
    }
}
