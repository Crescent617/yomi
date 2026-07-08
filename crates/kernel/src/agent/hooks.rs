use crate::event::ToolEvent;
use crate::hooks::{HookContext, HookRegistry, HookResult};
use crate::tools::executor::ToolExecutionResult;
use crate::types::{Message, MessageId, ToolCall};
use std::path::PathBuf;

/// Run `PreToolUse` hooks over a list of approved calls.
///
/// Calls that are blocked by a hook are moved into `denied_results` (with the
/// same pre-generated `MessageId` so `Start`/`End` events share one ID).
/// Returns the subset of calls that remain approved, potentially with modified
/// arguments when a hook rewrites the input.
pub async fn run_pre_tool_hooks(
    session_id: &str,
    working_dir: &PathBuf,
    hook_registry: &HookRegistry,
    toolcalls: Vec<ToolCall>,
    tool_message_ids: &std::collections::BTreeMap<String, MessageId>,
    denied_results: &mut Vec<ToolExecutionResult>,
) -> Vec<ToolCall> {
    if hook_registry.is_empty() {
        return toolcalls;
    }
    let mut approved = Vec::new();
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
                    let message_id = tool_message_ids
                        .get(&call.id)
                        .cloned()
                        .unwrap_or_else(MessageId::new);
                    let message = Message::tool_result(
                        message_id.clone(),
                        call.id.clone(),
                        final_reason.clone(),
                    );
                    denied_results.push(ToolExecutionResult {
                        tool_call_id: call.id.clone(),
                        message_id: message_id.clone(),
                        event: ToolEvent::End {
                            message_id,
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            content_blocks: vec![crate::types::ToolOutputBlock::Text {
                                text: final_reason,
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
                    approved.push(modified);
                }
            },
            _ => approved.push(call),
        }
    }
    approved
}

/// Run the `PostToolUse` hook for a single tool result.
///
/// Returns `(result, continue_session, context_messages)`.
/// `context_messages` are additional strings to be injected as user messages
/// (matching Claude Code's `additionalContext` behaviour).
pub async fn run_post_tool_hook_single(
    session_id: &str,
    working_dir: &PathBuf,
    hook_registry: &HookRegistry,
    mut result: ToolExecutionResult,
    tool_name: &str,
) -> (ToolExecutionResult, bool, Vec<String>) {
    if hook_registry.is_empty() {
        return (result, true, Vec::new());
    }

    let mut hook_tool_output = crate::types::ToolOutput::text(result.message.text_content());
    hook_tool_output.is_error = matches!(result.event, ToolEvent::End { is_error: true, .. });

    let ctx = HookContext::post_tool(
        session_id,
        tool_name,
        &result.tool_call_id,
        working_dir,
        &hook_tool_output,
    );
    let (hook_result, hook_contexts) = hook_registry.run_post_tool(&ctx).await;

    let mut continue_session = true;

    if let HookResult::PostTool(decision) = hook_result {
        if !decision.continue_session {
            continue_session = false;
        }

        let mut final_text = result.message.text_content();
        let mut modified = false;

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
            result.event = rewrite_end_event(result.event, tool_name, final_text);
        }
    }

    (result, continue_session, hook_contexts)
}

/// Replace the text content of a `ToolEvent::End` with `new_text`.
/// Other event variants are returned unchanged.
fn rewrite_end_event(event: ToolEvent, tool_name: &str, new_text: String) -> ToolEvent {
    match event {
        ToolEvent::End {
            message_id,
            tool_id,
            elapsed_ms,
            is_error,
            ..
        } => ToolEvent::End {
            message_id,
            tool_id,
            tool_name: tool_name.to_string(),
            content_blocks: vec![crate::types::ToolOutputBlock::Text { text: new_text }],
            elapsed_ms,
            is_error,
        },
        other => other,
    }
}
