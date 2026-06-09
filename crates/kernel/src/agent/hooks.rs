use crate::event::ToolEvent;
use crate::hooks::{HookContext, HookRegistry, HookResult};
use crate::tools::executor::ToolExecutionResult;
use crate::types::{AgentId, Message, MessageId, ToolCall};
use std::path::PathBuf;

/// Run `PreToolUse` hooks over approved calls.
///
/// Returns the still-approved calls. Any call that is blocked by a hook is
/// converted into a denied result (added to `denied_results`) with the hook's
/// `context` merged into the error text so that the user/LLM can see it.
/// When a hook allows the call, its `context` is ignored.
pub async fn run_pre_tool_hooks(
    agent_id: &AgentId,
    session_id: &str,
    working_dir: &PathBuf,
    hook_registry: &HookRegistry,
    toolcalls: Vec<ToolCall>,
    denied_results: &mut Vec<ToolExecutionResult>,
) -> Vec<ToolCall> {
    if hook_registry.is_empty() {
        return toolcalls;
    }
    let mut pre_approved = Vec::new();
    for call in toolcalls {
        let ctx = HookContext::pre_tool(
            session_id,
            &call.name,
            &call.id,
            working_dir,
            call.arguments.clone(),
        );
        let (result, hook_contexts) = hook_registry.run_pre_tool(&ctx).await;
        match result {
            HookResult::PreTool(decision) => match decision.action {
                crate::hooks::PreToolAction::Block => {
                    let reason = decision
                        .reason
                        .unwrap_or_else(|| format!("Blocked by hook for tool '{}'", call.name));
                    let final_reason = if hook_contexts.is_empty() {
                        reason
                    } else {
                        let mut parts = hook_contexts;
                        parts.push(reason);
                        parts.join("\n\n")
                    };
                    let message_id = MessageId::new();
                    let message = Message::tool_result(
                        message_id.clone(),
                        call.id.clone(),
                        final_reason.clone(),
                    );
                    denied_results.push(ToolExecutionResult {
                        tool_call_id: call.id.clone(),
                        message_id: message_id.clone(),
                        event: ToolEvent::End {
                            agent_id: agent_id.clone(),
                            message_id,
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            content_blocks: vec![crate::types::ToolOutputBlock::Text {
                                text: final_reason.clone(),
                            }],
                            elapsed_ms: 0,
                            is_error: true,
                        },
                        message,
                    });
                }
                crate::hooks::PreToolAction::Allow => {
                    let mut modified = call;
                    if let Some(new_args) = decision.updated_input {
                        modified.arguments = new_args;
                    }
                    pre_approved.push(modified);
                }
            },
            _ => {
                pre_approved.push(call);
            }
        }
    }
    pre_approved
}

/// Run `PostToolUse` hooks over executed results.
///
/// Returns `(modified_results, continue_session, context_messages)`.
/// `context_messages` are additional context strings that should be injected
/// into the conversation as independent messages (aligned with Claude Code's
/// `additionalContext` behaviour).
/// If any hook sets `continue_session: false`, the overall result is `false`.
pub async fn run_post_tool_hooks(
    agent_id: &AgentId,
    session_id: &str,
    working_dir: &PathBuf,
    hook_registry: &HookRegistry,
    results: Vec<ToolExecutionResult>,
    tool_calls: &[ToolCall],
) -> (Vec<ToolExecutionResult>, bool, Vec<String>) {
    if hook_registry.is_empty() {
        return (results, true, Vec::new());
    }
    let mut post_results = Vec::new();
    let mut continue_session = true;
    let mut contexts = Vec::new();
    for mut result in results {
        let tool_name = tool_calls
            .iter()
            .find(|c| c.id == result.tool_call_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let mut hook_tool_output = crate::types::ToolOutput::text(result.message.text_content());
        hook_tool_output.is_error = matches!(result.event, ToolEvent::End { is_error: true, .. });
        let ctx = HookContext::post_tool(
            session_id,
            &tool_name,
            &result.tool_call_id,
            working_dir,
            &hook_tool_output,
        );
        let (hook_result, hook_contexts) = hook_registry.run_post_tool(&ctx).await;
        contexts.extend(hook_contexts);
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

            if modified {
                let message_id = result.message_id.clone();
                result.message =
                    Message::tool_result(message_id.clone(), &result.tool_call_id, &final_text);
                result.event = match result.event {
                    ToolEvent::End {
                        message_id,
                        tool_id,
                        elapsed_ms,
                        mut content_blocks,
                        is_error,
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
                        ToolEvent::End {
                            agent_id: agent_id.clone(),
                            message_id,
                            tool_id,
                            tool_name: tool_name.clone(),
                            content_blocks,
                            elapsed_ms,
                            is_error,
                        }
                    }
                    other => other,
                };
            }
        }
        post_results.push(result);
    }
    (post_results, continue_session, contexts)
}
