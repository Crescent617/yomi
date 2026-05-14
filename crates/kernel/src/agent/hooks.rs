use crate::event::ToolEvent;
use crate::hooks::{HookContext, HookRegistry, HookResult};
use crate::tools::executor::ToolExecutionResult;
use crate::types::{AgentId, Message, ToolCall};
use std::fmt::Write as _;
use std::path::PathBuf;

/// Run `PreToolUse` hooks over approved calls.
///
/// Returns the still-approved calls and a list of context strings to inject
/// into the conversation.
pub async fn run_pre_tool_hooks(
    agent_id: &AgentId,
    session_id: &str,
    working_dir: &PathBuf,
    hook_registry: &HookRegistry,
    msg_count: usize,
    approved_calls: Vec<ToolCall>,
    denied_results: &mut Vec<ToolExecutionResult>,
) -> (Vec<ToolCall>, Vec<String>) {
    let mut pre_approved = Vec::new();
    let mut allow_contexts = Vec::new();
    for call in approved_calls {
        let ctx = HookContext::pre_tool(
            session_id,
            agent_id.to_string(),
            &call.name,
            &call.id,
            working_dir,
            call.arguments.clone(),
            msg_count,
        );
        let (result, contexts) = hook_registry.run_pre_tool(&ctx).await;
        match result {
            HookResult::PreTool(decision) => match decision.action {
                crate::hooks::PreToolAction::Block => {
                    let mut reason = decision
                        .reason
                        .unwrap_or_else(|| format!("Blocked by hook for tool '{}'", call.name));
                    if !contexts.is_empty() {
                        reason.push_str("\n\nHook context:\n");
                        for ctx in &contexts {
                            let _ = writeln!(reason, "- {ctx}");
                        }
                    }
                    denied_results.push(ToolExecutionResult {
                        tool_call_id: call.id.clone(),
                        event: ToolEvent::Error {
                            agent_id: agent_id.clone(),
                            tool_id: call.id.clone(),
                            error: reason.clone(),
                            content_blocks: Vec::new(),
                            elapsed_ms: 0,
                        },
                        message: Message::tool_result(call.id, reason),
                    });
                }
                crate::hooks::PreToolAction::Allow => {
                    let mut modified = call;
                    if let Some(new_args) = decision.updated_input {
                        modified.arguments = new_args;
                    }
                    pre_approved.push(modified);
                    allow_contexts.extend(contexts);
                }
            },
            _ => pre_approved.push(call),
        }
    }
    (pre_approved, allow_contexts)
}

/// Run `PostToolUse` hooks over executed results.
///
/// Returns `(modified_results, continue_session)`.
/// If any hook sets `continue_session: false`, the overall result is `false`.
pub async fn run_post_tool_hooks(
    agent_id: &AgentId,
    session_id: &str,
    working_dir: &PathBuf,
    hook_registry: &HookRegistry,
    msg_count: usize,
    results: Vec<ToolExecutionResult>,
    tool_calls: &[ToolCall],
) -> (Vec<ToolExecutionResult>, bool) {
    let mut post_results = Vec::new();
    let mut continue_session = true;
    for mut result in results {
        let tool_name = tool_calls
            .iter()
            .find(|c| c.id == result.tool_call_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let mut hook_tool_output =
            crate::types::ToolOutput::text(result.message.text_content());
        hook_tool_output.is_error = matches!(result.event, ToolEvent::Error { .. });
        let ctx = HookContext::post_tool(
            session_id,
            agent_id.to_string(),
            &tool_name,
            &result.tool_call_id,
            working_dir,
            &hook_tool_output,
            msg_count,
        );
        let (hook_result, contexts) = hook_registry.run_post_tool(&ctx).await;
        if let HookResult::PostTool(decision) = hook_result {
            if !decision.continue_session {
                continue_session = false;
            }
            let mut modified = false;
            let mut final_text = result.message.text_content();

            if let Some(updated) = decision.updated_output {
                final_text = updated;
                modified = true;
            }
            if let Some(append) = decision.append_output {
                if !final_text.is_empty() {
                    final_text.push('\n');
                }
                final_text.push_str(&append);
                modified = true;
            }
            if !contexts.is_empty() {
                let ctx_text = contexts.join("\n");
                if !final_text.is_empty() {
                    final_text.push_str("\n\n");
                }
                final_text.push_str("[Hook context]\n");
                final_text.push_str(&ctx_text);
                modified = true;
            }

            if modified {
                result.message = Message::tool_result(&result.tool_call_id, &final_text);
                result.event = match result.event {
                    ToolEvent::Output {
                        tool_id,
                        elapsed_ms,
                        mut content_blocks,
                        ..
                    } => {
                        if let Some(crate::types::ToolOutputBlock::Text {
                            text: ref mut existing,
                        }) = content_blocks.last_mut()
                        {
                            existing.clone_from(&final_text);
                        } else {
                            content_blocks.push(crate::types::ToolOutputBlock::Text {
                                text: final_text.clone(),
                            });
                        }
                        ToolEvent::Output {
                            agent_id: agent_id.clone(),
                            tool_id,
                            tool_name: tool_name.clone(),
                            output: final_text,
                            content_blocks,
                            elapsed_ms,
                        }
                    }
                    ToolEvent::Error {
                        tool_id,
                        elapsed_ms,
                        mut content_blocks,
                        ..
                    } => {
                        if let Some(crate::types::ToolOutputBlock::Text {
                            text: ref mut existing,
                        }) = content_blocks.last_mut()
                        {
                            existing.clone_from(&final_text);
                        } else {
                            content_blocks.push(crate::types::ToolOutputBlock::Text {
                                text: final_text.clone(),
                            });
                        }
                        ToolEvent::Error {
                            agent_id: agent_id.clone(),
                            tool_id,
                            error: final_text,
                            content_blocks,
                            elapsed_ms,
                        }
                    }
                    other => other,
                };
            }
        }
        post_results.push(result);
    }
    (post_results, continue_session)
}
