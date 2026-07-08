//! Tool execution logic for `Agent`.
//!
//! Extracted from `agent.rs` so that the main loop stays readable.
//!
//! # Flow
//!
//! ```text
//! handle_execute_tool()
//!   └─ collect_pending_tool_calls()   — skip already-completed calls (resume)
//!   └─ execute_tools()
//!        ├─ emit ToolEvent::Start     — for every pending call
//!        ├─ permission check          — split into approved / denied
//!        ├─ pre hooks                 — may block further calls → denied
//!        ├─ emit + save denied        — immediately persisted
//!        ├─ JoinSet (parallel exec)
//!        │    └─ join_next() loop
//!        │         ├─ post hook       — per result
//!        │         ├─ emit End event
//!        │         └─ push_message()  — immediately persisted
//!        └─ inject post-hook contexts as user messages
//! ```

use super::super::hooks::{run_post_tool_hook_single, run_pre_tool_hooks};
use crate::event::{Event, ToolEvent};
use crate::tools::executor::{
    build_tool_result, execute_single_tool, log_tool_result, ToolExecutionResult,
};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{Message, MessageId, Role, ToolCall};
use futures::FutureExt;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use tokio::task::JoinSet;
use tracing::Instrument;

use super::super::AgentError;
use super::Agent;
use crate::agent::AgentState;

impl Agent {
    /// Entry point called from the main loop when in `ExecutingTool` state.
    ///
    /// Skips tool calls whose results are already present in the message buffer
    /// (recovery after a mid-batch process kill), then delegates to
    /// `execute_tools` for the remaining pending calls.
    pub(super) async fn handle_execute_tool(&mut self) -> Result<(), AgentError> {
        if self.cancel_token.is_cancelled() {
            return Err(AgentError::Cancelled("tool execution".into()));
        }

        let Some((all_calls, pending)) = self.pending_tool_calls() else {
            // No resumable tool batch found — nothing to do.
            self.context.transition_to(AgentState::Streaming);
            return Ok(());
        };

        if pending.is_empty() {
            // All results already persisted (pure recovery path).
            self.context.transition_to(AgentState::Streaming);
            return Ok(());
        }

        let continue_session = self.execute_tools(&all_calls, pending).await?;

        if continue_session {
            self.context.transition_to(AgentState::Streaming);
        } else {
            tracing::info!("stopping after tool execution (hook requested)");
            self.context.transition_to(AgentState::Idle);
        }
        Ok(())
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Find the pending tool calls that still need to be executed.
    ///
    /// Looks for the last Assistant message that carries `tool_calls`, then
    /// verifies the messages that follow it contain no User message (which
    /// would mean we are already in a new turn and should not resume here).
    ///
    /// Returns `None` when there is nothing to execute:
    /// - no Assistant message with `tool_calls` found, or
    /// - a User message appears after that Assistant (already moved on).
    ///
    /// Otherwise returns `(all_calls, pending_calls)` where `pending_calls`
    /// is `all_calls` minus those that already have a matching Tool result.
    pub(super) fn pending_tool_calls(&self) -> Option<(Vec<ToolCall>, Vec<ToolCall>)> {
        let messages = self.message_buffer.messages();

        // Find the last Assistant message that has tool_calls.
        let (assistant_idx, all_calls) = messages.iter().enumerate().rev().find_map(|(i, m)| {
            m.tool_calls
                .as_ref()
                .filter(|tc| !tc.is_empty())
                .map(|tc| (i, tc.clone()))
        })?;

        // If any User message appears after that Assistant, we are in a new
        // turn — do not resume the old tool batch.
        let has_user_after = messages[assistant_idx + 1..]
            .iter()
            .any(|m| m.role == Role::User);
        if has_user_after {
            return None;
        }

        // Collect tool_call_ids that already have a result.
        let already_done: HashSet<&str> = messages[assistant_idx + 1..]
            .iter()
            .filter(|m| m.role == Role::Tool)
            .filter_map(|m| m.tool_call_id.as_deref())
            .collect();

        let pending: Vec<ToolCall> = all_calls
            .iter()
            .filter(|c| !already_done.contains(c.id.as_str()))
            .cloned()
            .collect();

        Some((all_calls, pending))
    }

    /// Execute `pending_calls` in parallel, with permission checks and hooks.
    ///
    /// `all_calls` is needed by post-hooks to look up tool names by call id.
    ///
    /// Returns `true` when the session should continue to `Streaming`,
    /// `false` when a post-hook requested a stop.
    async fn execute_tools(
        &mut self,
        all_calls: &[ToolCall],
        pending_calls: Vec<ToolCall>,
    ) -> Result<bool, AgentError> {
        tracing::info!("Executing {} tool(s) in parallel", pending_calls.len());

        // Pre-assign MessageIds so Start/End events share one stable ID.
        let message_ids = assign_message_ids(&pending_calls);

        self.emit_start_events(&pending_calls, &message_ids);

        // Permission check → approved / denied split.
        let (approved, mut denied) = self.check_permissions(&pending_calls, &message_ids).await;

        // Pre hooks may block further calls.
        let approved = run_pre_tool_hooks(
            &self.session_id.0,
            &self.working_dir,
            &self.hook_registry,
            approved,
            &message_ids,
            &mut denied,
        )
        .await;

        // Persist denied results immediately.
        self.emit_and_save_results(denied);

        // Run approved calls concurrently, persisting each result as it arrives.
        let continue_session = self.run_parallel(approved, all_calls, &message_ids).await?;

        Ok(continue_session)
    }

    /// Emit `ToolEvent::Start` for every pending call.
    fn emit_start_events(&self, calls: &[ToolCall], message_ids: &BTreeMap<String, MessageId>) {
        for call in calls {
            let message_id = message_ids[&call.id].clone();
            let args_str = serde_json::to_string(&call.arguments).ok();
            self.emit(Event::Tool(ToolEvent::Start {
                message_id,
                tool_id: call.id.clone(),
                tool_name: call.name.clone(),
                arguments: args_str,
            }));
        }
    }

    /// Run the permission checker; returns `(approved, denied_results)`.
    async fn check_permissions(
        &self,
        calls: &[ToolCall],
        message_ids: &BTreeMap<String, MessageId>,
    ) -> (Vec<ToolCall>, Vec<ToolExecutionResult>) {
        let perm =
            crate::permission::check_tool_permissions(calls, self.permission_checker.as_deref())
                .await;

        let denied = perm
            .denied
            .into_iter()
            .map(|(call_id, error_msg)| {
                let message_id = message_ids[&call_id].clone();
                let tool_name = calls
                    .iter()
                    .find(|c| c.id == call_id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                let output = crate::types::ToolOutput::error(error_msg);
                let (event, message) = build_tool_result(
                    &call_id,
                    &tool_name,
                    &output,
                    0,
                    message_id.clone(),
                    self.max_tool_output_length,
                );
                ToolExecutionResult {
                    tool_call_id: call_id,
                    message_id,
                    event,
                    message,
                }
            })
            .collect();

        (perm.approved, denied)
    }

    /// Emit + persist a batch of results immediately.
    fn emit_and_save_results(&mut self, results: Vec<ToolExecutionResult>) {
        for result in results {
            log_tool_result(&result);
            self.emit(Event::Tool(result.event));
            self.push_message(result.message);
        }
    }

    /// Spawn all `approved` calls in a `JoinSet`, then harvest results one by
    /// one so each is post-hook'd, emitted, and persisted as soon as it's done.
    ///
    /// Returns `false` if any post-hook requested the session to stop.
    async fn run_parallel(
        &mut self,
        approved: Vec<ToolCall>,
        all_calls: &[ToolCall],
        message_ids: &BTreeMap<String, MessageId>,
    ) -> Result<bool, AgentError> {
        if approved.is_empty() {
            return Ok(true);
        }

        let cancel_token = self.create_runtime_token();
        let mut join_set = self.spawn_tool_tasks(approved, message_ids, &cancel_token);

        let mut continue_session = true;
        let mut hook_contexts_acc = Vec::new();

        loop {
            tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    join_set.abort_all();
                    // Drain completed tasks before bailing so we don't lose
                    // results that finished before the abort propagated.
                    while let Some(outcome) = join_set.join_next().await {
                        if let Some(result) = Self::unwrap_join_outcome(outcome) {
                            self.emit_and_save_results(vec![result]);
                        }
                    }
                    return Err(AgentError::Cancelled("tool execution".into()));
                }
                outcome = join_set.join_next() => {
                    match outcome {
                        None => break,   // JoinSet exhausted
                        Some(r) => {
                            let Some(result) = Self::unwrap_join_outcome(r) else { continue; };
                            let (result, cont, ctxs) = self.apply_post_hook(result, all_calls).await;
                            if !cont { continue_session = false; }
                            log_tool_result(&result);
                            self.emit(Event::Tool(result.event));
                            self.push_message(result.message);
                            hook_contexts_acc.extend(ctxs);
                        }
                    }
                }
            }
        }

        // Inject hook contexts only after all tool results are persisted,
        // keeping the tool-call chain contiguous so sanitize() won't strip it.
        self.inject_hook_contexts(hook_contexts_acc);

        Ok(continue_session)
    }

    /// Spawn one task per approved call.  All cloning of registry/ctx data
    /// happens here so the tasks are `'static`.
    fn spawn_tool_tasks(
        &self,
        calls: Vec<ToolCall>,
        message_ids: &BTreeMap<String, MessageId>,
        cancel_token: &tokio_util::sync::CancellationToken,
    ) -> JoinSet<ToolExecutionResult> {
        let mut join_set = JoinSet::new();
        let max_len = self.max_tool_output_length;

        for call in calls {
            let tool_opt = self.tool_registry.get(&call.name);
            if tool_opt.is_none() {
                tracing::error!(
                    "Tool '{}' not found in registry. Available: {:?}",
                    call.name,
                    self.tool_registry.list()
                );
            }

            let call_id = call.id.clone();
            let call_name = call.name.clone();
            let arguments = call.arguments.clone();
            let message_id = message_ids[&call.id].clone();
            let session_id = self.session_id.0.to_string();
            let working_dir = self.working_dir.clone();
            let cancel = cancel_token.clone();
            let turn = self.current_turn.clone();

            join_set.spawn(
                async move {
                    // SAFETY: `run_single_tool` only touches stack-local data and
                    // the `ToolExecCtx` (which is owned for this task). All `Tool`
                    // implementations are expected to be panic-safe (no shared
                    // mutable state without poisoning protection). We catch the
                    // panic here so one bad tool doesn't crash the whole agent.
                    let result =
                        std::panic::AssertUnwindSafe(run_single_tool(RunSingleToolParams {
                            tool_opt,
                            call_id: &call_id,
                            call_name: &call_name,
                            arguments,
                            message_id: message_id.clone(),
                            cancel_token: cancel,
                            working_dir,
                            session_id,
                            turn,
                            max_tool_output_length: max_len,
                        }))
                        .catch_unwind()
                        .await;

                    match result {
                        Ok(r) => r,
                        Err(e) => {
                            let msg = panic_message(&e);
                            tracing::error!("Tool '{}' panicked: {}", call_name, msg);
                            let output =
                                crate::types::ToolOutput::error(format!("Tool panicked: {msg}"));
                            let (event, message) = build_tool_result(
                                &call_id,
                                &call_name,
                                &output,
                                0,
                                message_id.clone(),
                                max_len,
                            );
                            ToolExecutionResult {
                                tool_call_id: call_id,
                                message_id,
                                message,
                                event,
                            }
                        }
                    }
                }
                .instrument(tracing::Span::current()),
            );
        }
        join_set
    }

    /// Unwrap a `JoinSet` outcome. Returns `None` only for cancelled tasks
    /// (panics are caught inside the task itself and turned into error results).
    fn unwrap_join_outcome(
        outcome: Result<ToolExecutionResult, tokio::task::JoinError>,
    ) -> Option<ToolExecutionResult> {
        match outcome {
            Ok(r) => Some(r),
            Err(e) if e.is_cancelled() => None,
            Err(e) => {
                // Should not happen since panics are caught inside the task,
                // but handle defensively.
                tracing::error!("Unexpected JoinError (not a panic or cancel): {e}");
                None
            }
        }
    }

    /// Apply the post-hook for a single result, looking up its tool name from
    /// `all_calls`.
    async fn apply_post_hook(
        &self,
        result: ToolExecutionResult,
        all_calls: &[ToolCall],
    ) -> (ToolExecutionResult, bool, Vec<String>) {
        let tool_name = all_calls
            .iter()
            .find(|c| c.id == result.tool_call_id)
            .map_or("", |c| c.name.as_str());
        run_post_tool_hook_single(
            &self.session_id.0,
            &self.working_dir,
            &self.hook_registry,
            result,
            tool_name,
        )
        .await
    }

    /// Inject post-hook context strings as independent user messages.
    ///
    /// These must be appended *after* all tool results so the tool-call chain
    /// stays contiguous and `sanitize()` won't strip it.
    fn inject_hook_contexts(&mut self, ctxs: Vec<String>) {
        for text in ctxs {
            self.push_user_message(Message::user(text));
        }
    }
}

// ── free functions ────────────────────────────────────────────────────────────

/// Assign a fresh `MessageId` to every tool call so that `Start` and `End`
/// events for the same call share one stable identifier.
fn assign_message_ids(calls: &[ToolCall]) -> BTreeMap<String, MessageId> {
    calls
        .iter()
        .map(|c| (c.id.clone(), MessageId::new()))
        .collect()
}

/// All parameters needed to execute one tool, bundled so the closure that
/// moves into `JoinSet::spawn` doesn't capture many individual values.
struct RunSingleToolParams<'a> {
    tool_opt: Option<Arc<dyn Tool>>,
    call_id: &'a str,
    call_name: &'a str,
    arguments: serde_json::Value,
    message_id: MessageId,
    cancel_token: tokio_util::sync::CancellationToken,
    working_dir: std::path::PathBuf,
    session_id: String,
    turn: Option<Arc<crate::agent::Turn>>,
    max_tool_output_length: usize,
}

/// Execute one tool and wrap the output in a `ToolExecutionResult`.
async fn run_single_tool(p: RunSingleToolParams<'_>) -> ToolExecutionResult {
    let start = std::time::Instant::now();

    let mut ctx = ToolExecCtx::new(p.call_id, &p.working_dir, p.session_id)
        .with_cancel_token(Some(p.cancel_token));
    ctx.message_id = p.message_id.clone();
    ctx.turn = p.turn;
    ctx.max_tool_output_length = p.max_tool_output_length;

    let output = match p.tool_opt {
        Some(tool) => execute_single_tool(tool, p.arguments, ctx).await,
        None => crate::types::ToolOutput::error(format!("Unknown tool: {}", p.call_name)),
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let (event, message) = build_tool_result(
        p.call_id,
        p.call_name,
        &output,
        elapsed_ms,
        p.message_id.clone(),
        p.max_tool_output_length,
    );

    ToolExecutionResult {
        tool_call_id: p.call_id.to_string(),
        message_id: p.message_id,
        message,
        event,
    }
}

/// Extract a human-readable string from a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return s.to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "unknown panic".to_string()
}
