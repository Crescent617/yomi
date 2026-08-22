mod tool_exec;

use super::message_buffer::MessageBuffer;
use super::{
    AgentError, AgentExecutionContext, AgentShared, AgentSpawnArgs, AgentState, CancelToken,
    InterceptCtx,
};
use crate::comms::{EventSink, Mailbox};
use crate::compactor::CompactionError;
use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, StopReason};
use crate::permission::Checker;
use crate::prompt::SystemPromptBuilder;
use crate::tools::{ToolRegistry, ToolRegistryConfig};
use crate::types::{ContentBlock, Message, MessageId, MessageTokenUsage, Role, SessionId};
use crate::FinishReason;
use futures::TryStreamExt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, Instrument};

/// Input messages that can be sent to an Agent
#[derive(Clone)]
pub enum AgentInput {
    /// User message with multi-modal content blocks
    User { content: Vec<ContentBlock> },
    /// Continue the agent from Idle to Streaming (`/continue` via RPC: GUI/CLI/TUI)
    Continue,
    /// Cancel current operation; the agent loop exits once the cancellation
    /// is observed (the next input respawns the agent with fresh context)
    Cancel,
    /// Steer message injected before the next streaming turn
    Steer(Vec<ContentBlock>),
    /// Permission response from user/TUI (handled directly by Checker via `input_bus`)
    PermissionResponse {
        req_id: String,
        approved: bool,
        remember: bool,
    },
    /// Shutdown the agent gracefully (for subagent/resource management)
    Shutdown,
    /// Force compaction of message buffer
    Compact,
    /// Rewind to a specific checkpoint
    Rewind {
        message_id: MessageId,
        target: crate::checkpoint::RewindTarget,
        /// Channel to send the result back
        result_tx: tokio::sync::mpsc::Sender<Result<(), AgentError>>,
    },
    /// Clear the agent's context (messages, file state, todos, persisted history)
    Clear,
    /// Response to an `ask_user` question (handled directly by `AskUserTool` via `input_bus`)
    AskUserResponse {
        req_id: String,
        response: crate::tools::AskUserResponse,
    },
}

pub struct Agent {
    shared: Arc<AgentShared>,
    message_buffer: MessageBuffer,
    event_sink: Arc<dyn EventSink>,
    context: AgentExecutionContext,
    cancel_token: CancelToken,
    session_id: SessionId,
    max_iterations: usize,
    // Tool registry - each agent has its own set of tools
    tool_registry: crate::tools::ToolRegistry,
    // Permission checker for tool execution
    permission_checker: Option<Arc<Checker>>,
    // Working directory for tool execution
    working_dir: std::path::PathBuf,
    /// Mailbox for receiving input messages
    mailbox: Arc<Mailbox>,
    /// Checkpoint store for persistence
    checkpoint_store: Arc<dyn crate::checkpoint::CheckpointStore>,
    /// Data directory for checkpoints
    data_dir: std::path::PathBuf,
    /// Current turn (contains tracked files, shared with tools)
    current_turn: Option<Arc<super::turn::Turn>>,
    /// Maximum tool output length in bytes
    max_tool_output_length: usize,
    /// Cached provider for the current model (resolved each turn)
    current_provider: Option<Arc<dyn crate::provider::Provider>>,
    /// Cached model config for the current model (resolved each turn)
    current_model_config: Option<Arc<crate::provider::ModelConfig>>,
    /// Whether the current user-initiated run already used its truncation recovery.
    auto_continue_used: bool,
}

impl Agent {
    pub async fn new(shared: &Arc<AgentShared>, args: AgentSpawnArgs) -> Self {
        let mailbox = args.mailbox;
        let cancel_token = args.cancel_token.clone().unwrap_or_default();

        let session_id = SessionId::from(args.session_id.clone());
        let event_bus = shared.event_bus.as_ref().map_or_else(
            || {
                // No event bus configured: use a no-op fallback to avoid panic
                Arc::new(crate::comms::EventBus::new()).handle(session_id.clone())
            },
            |eb| eb.handle(session_id.clone()),
        );

        let session_id_for_state_change = session_id.clone();
        let event_bus_for_state_change = event_bus.clone();
        let context = AgentExecutionContext::new(
            AgentState::Idle,
            Some(Box::new(move |state: AgentState| {
                if let Err(e) = event_bus_for_state_change.try_send(crate::event::Envelope::new(
                    session_id_for_state_change.clone(),
                    Event::Agent(AgentEvent::StateChanged { state }),
                )) {
                    tracing::warn!("Failed to send StateChanged event for {:?}: {}", state, e);
                }
            })),
        );

        let system_prompt = SystemPromptBuilder::new()
            .base_prompt(&args.base_prompt)
            .with_skills(&args.skills)
            .with_working_dir(&args.working_dir)
            .with_session_id(&args.session_id)
            .build()
            .await;

        tracing::debug!("spawning with system prompt: {}", system_prompt);
        let mut messages: Vec<Arc<Message>> = vec![Arc::new(Message::system(system_prompt))];
        messages.extend(args.history.into_iter().filter(|m| m.role != Role::System));
        let message_buffer = MessageBuffer::from_arc_messages(&messages);

        let shared = shared.clone();

        let mut tool_registry = ToolRegistry::new().with_standard_tools(
            ToolRegistryConfig {
                shared: &shared,
                event_bus: &event_bus,
                session_id: &args.session_id,
                input_bus: args.input_bus.as_ref(),
                file_state_store: None,
                tool_blocklist: args.tool_blocklist.clone(),
                flags: args.tool_flags,
            }
            .with_file_state_store(args.file_state_store.clone()),
        );

        // wire 外部扩展工具（spawn 时快照）：收口与内建一致——
        // blocklist 同样生效，与内建/已注册撞名让位记 warn。
        for tool in args.ext_tools {
            let name = tool.name().to_string();
            if !crate::tools::passes_blocklist(&name, &args.tool_blocklist) {
                tracing::info!("Extension tool '{name}' blocked by blocklist pattern");
                continue;
            }
            if tool_registry.get(&name).is_some() {
                tracing::warn!("Extension tool '{name}' shadows a registered tool; skipped");
                continue;
            }
            tool_registry.register_arc(tool);
        }

        let permission_checker = shared
            .permission_state
            .as_ref()
            .zip(args.input_bus.as_ref())
            .map(|(state, input_bus)| {
                Arc::new(Checker::new(
                    state.clone(),
                    event_bus.clone(),
                    Arc::clone(input_bus),
                    session_id.clone(),
                ))
            });

        let checkpoint_store = shared.checkpoint_store.clone().unwrap_or_else(|| {
            Arc::new(crate::checkpoint::FilesystemCheckpointStore::new(
                &shared.data_dir,
            ))
        });

        let data_dir = shared.data_dir.clone();

        Self {
            shared,
            message_buffer,
            event_sink: Arc::new(event_bus.clone()) as Arc<dyn EventSink>,
            context,
            cancel_token,
            session_id,
            max_iterations: args.max_iterations,
            tool_registry,
            permission_checker,
            working_dir: args.working_dir,
            mailbox,
            checkpoint_store,
            data_dir,
            current_turn: None,
            max_tool_output_length: args.max_tool_output_length,
            current_provider: None,
            current_model_config: None,
            auto_continue_used: false,
        }
    }

    /// Emit an event through the event sink.
    fn emit(&self, event: Event) {
        self.event_sink
            .emit(crate::event::Envelope::new(self.session_id.clone(), event));
    }

    /// Create a runtime `CancellationToken` linked to the Agent's custom `CancelToken`.
    ///
    /// This bridges the Agent layer (with reset support) to the Runtime layer
    /// (using tokio native `CancellationToken`).
    fn create_runtime_token(&self) -> tokio_util::sync::CancellationToken {
        // Obtain the parent agent's tokio CancellationToken directly
        // No bridge task needed since CancelToken internally wraps a CancellationToken
        self.cancel_token.runtime_token()
    }

    /// Clear the agent's context (messages, file state, todos, persisted history).
    /// Keeps the system prompt message.
    pub(crate) async fn handle_clear(&mut self) {
        tracing::info!("clearing agent context");

        // 1. Keep system prompt (first message if it's system role)
        let system_msg = self
            .message_buffer
            .messages()
            .first()
            .filter(|m| m.role == Role::System)
            .cloned();

        // 2. Clear message buffer and push system prompt back
        self.message_buffer.clear();
        if let Some(sys) = system_msg {
            self.message_buffer.push_arc(sys.clone());
        }

        // 3. Clear file state store
        if let Some(ref store) = self.shared.file_state_store {
            store.clear().await;
        }

        // 4. Clear todo storage
        if let Some(ref store) = self.shared.todo_storage {
            if let Err(e) = store.clear(&self.session_id.0).await {
                tracing::warn!("failed to clear todo storage: {}", e);
            }
        }

        // 5. Replace persisted messages with just system prompt
        self.emit(Event::Internal(
            crate::event::InternalEvent::MessageReplaced {
                messages: self.message_buffer.messages().to_vec(),
            },
        ));
    }

    pub async fn start_loop(mut self) -> Result<(), AgentError> {
        let result = async move {

            // On startup, check if there are pending tool calls in the loaded
            // history (recovery after a mid-batch process kill).
            // TODO: 先注释掉这个功能，想清楚再加回来，目前不完备，比如cancel之后其实不需要这个行为
            // if self.pending_tool_calls().is_some() {
            //     tracing::info!("resuming interrupted tool execution from history");
            //     // Ensure the turn is recreated so that tools can track file edits
            //     // during the resumed batch (same user message as before the kill).
            //     self.start_turn_if_needed().await;
            //     self.context.transition_to(AgentState::ExecutingTool);
            // }

            loop {
                let state = self.context.current_state();

                if self.max_iterations > 0
                    && self.context.iteration_count() >= self.max_iterations
                    && self.context.current_state() == AgentState::Streaming
                {
                    tracing::warn!(
                        "reached max iterations during streaming, cancelling and returning to waiting for input"
                    );
                    // Notify TUI that max iterations reached
                    self.emit(Event::Agent(AgentEvent::Lifecycle {
                        state: AgentStatus::Stopped {
                            reason: StopReason::MaxIterations {
                                reached: self.max_iterations,
                            },
                        },
                    }));
                    self.context.transition_to(AgentState::Idle);
                    continue;
                }

                // Note: cancel is handled during streaming via select!, not here
                let result = match state {
                    AgentState::Idle => {
                        // Once cancelled, the agent exits instead of resetting:
                        // the next input respawns it with freshly assembled
                        // context (system prompt, skills, project memory).
                        if self.cancel_token.is_cancelled() {
                            tracing::info!("agent cancelled, exiting loop");
                            break;
                        }
                        self.context.reset_iteration();
                        // steer 插队
                        let steers = self.mailbox.try_pull_steer(20).await;
                        if !steers.is_empty() {
                            self.emit_mailbox_changed().await;
                            self.inject_user_message(steers, true).await?;
                            continue; // inject_user_message already transitioned to Streaming
                        }
                        // 取一条普通消息
                        match self.mailbox.try_pull(1).await.into_iter().next() {
                            Some(input) => {
                                self.emit_mailbox_changed().await;
                                self.handle_input(input).await
                            }
                            None => {
                                tokio::select! {
                                    biased;
                                    () = self.cancel_token.cancelled() => {
                                        tracing::info!("agent cancelled while idle, exiting loop");
                                        break;
                                    }
                                    () = self.mailbox.wait_for_mail() => {
                                        continue;
                                    }
                                    () = tokio::time::sleep(std::time::Duration::from_mins(5)) => {
                                        if !self.mailbox.is_empty() {
                                            continue;
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    AgentState::Streaming => {
                        // Start new turn when entering Streaming
                        self.start_turn_if_needed().await;
                        tracing::debug!("starting streaming");
                        // Notify UI that streaming has started
                        self.emit(Event::Agent(AgentEvent::Lifecycle {
                            state: AgentStatus::Running,
                        }));
                        // steer 插队
                        let steers = self.mailbox.try_pull_steer(20).await;
                        if !steers.is_empty() {
                            self.emit_mailbox_changed().await;
                            self.inject_user_message(steers, true).await?;
                        }
                        self.handle_streaming_with_retry().await
                    }
                    AgentState::ExecutingTool => {
                        tracing::debug!("executing tools");
                        self.handle_execute_tool().await
                    }
                    AgentState::Compacting => {
                        tracing::warn!("Unexpected Compacting state in main loop; returning to Idle");
                        self.context.transition_to(AgentState::Idle);
                        continue;
                    }
                };

                // Handle state transition after execution
                if let Err(e) = result {
                    if e.is_shutdown() {
                        break;
                    }
                    tracing::warn!("error in main loop: {}", e);
                    if let AgentError::Cancelled(ctx) = &e {
                        if let Err(e) = self.handle_cancel(ctx).await {
                            tracing::warn!("Failed to handle cancel: {}", e);
                        }
                    } else {
                        let phase = match state {
                            AgentState::Idle => crate::event::ErrorPhase::Idle,
                            AgentState::Streaming => crate::event::ErrorPhase::Streaming,
                            AgentState::ExecutingTool => crate::event::ErrorPhase::ToolExecution,
                            AgentState::Compacting => crate::event::ErrorPhase::Compaction,
                        };
                        self.emit_error(phase, &e.to_string(), false);

                        // Recover to Idle for non-Idle states
                        if self.context.current_state() != AgentState::Idle {
                            self.context.transition_to(AgentState::Idle);
                        }
                    }
                }

                self.complete_turn_if_needed(state).await;
                self.context.increment_iteration();
            }

            Ok(())
        }.await;

        result
    }

    /// Handle cancellation - sends Cancelled event, transitions state, returns Ok(())
    async fn handle_cancel(&mut self, context: &str) -> Result<(), AgentError> {
        tracing::info!("{} cancelled", context);
        self.mark_interrupted("cancelled").await;
        // Emit cancellation event with operation name
        self.emit_operation_cancelled(context);
        self.context.transition_to(AgentState::Idle);
        Ok(())
    }

    /// Append a user-role interruption marker to the history (Claude Code
    /// 同款 `[Request interrupted by user]`): after an abort the model must
    /// not assume its last actions completed. The marker also trips the
    /// has_user_after guard in `pending_tool_calls`, so an interrupted tool
    /// batch is never silently re-executed after a respawn.
    ///
    /// Persistence is a direct store append, not the MessageAdded event
    /// bus: on the daemon-shutdown path the conductor (the bus's only
    /// persister) is already torn down when agents observe the cancel, so a
    /// bus-published marker would be lost on exactly the path it exists
    /// for. The buffer is updated in place to avoid a double persist.
    /// Metadata flags it for UIs to render as a divider/system line.
    ///
    /// 直写前先排空持久化池（2026-08-22 复审 should-fix）：jsonl
    /// append 非原子，直写与池 worker 的 drain/在飞写并发写同一文
    /// 件会交错出坏行（`read_lines` 静默跳过 → 双丢）；排空后直写
    /// 还把 marker 顺序顺带改正（落在既有消息之后）。**残余窗口**
    /// （fresh-eyes 复审）：`wait_drained` 只覆盖**已 dispatch**
    /// 的写——仍堵在 conductor bus 队列里、尚未分发的
    /// `MessageAdded` 会在 marker 直写之后才入池落盘，marker 顺序
    /// 与并发写在该窗口内仍可能存在（与旧码 inline 时代同型）。
    /// 上界与降级见 `wait_drained`。
    async fn mark_interrupted(&mut self, reason: &str) {
        let mut msg = Message::user(format!("[interrupted: {reason}]"));
        msg.metadata = Some(std::collections::HashMap::from([(
            crate::types::INTERRUPTED_META_KEY.to_string(),
            "true".to_string(),
        )]));
        if let Some(store) = &self.shared.message_store {
            if let Some(ref pool) = self.shared.persist_pool {
                crate::kernel::persist_pool::wait_drained(
                    pool,
                    &self.session_id,
                    "interruption marker",
                )
                .await;
            }
            if let Err(e) = store.append(&self.session_id.0, &[msg.clone()]).await {
                tracing::warn!("failed to persist interruption marker: {e}");
            }
        }
        self.message_buffer.push_arc(Arc::new(msg));
    }

    /// Complete current turn if transitioning from non-Idle to Idle.
    async fn complete_turn_if_needed(&mut self, from_state: AgentState) {
        if from_state == AgentState::Idle {
            return;
        }
        if self.context.current_state() != AgentState::Idle {
            return;
        }
        if let Some(turn) = self.current_turn.take() {
            if let Err(e) = turn.complete().await {
                tracing::warn!("Failed to complete turn: {}", e);
            }
        }
    }

    /// Helper to emit `AgentEvent::Lifecycle(Stopped(Failed))` and return Ok.
    /// This stops the agent gracefully without entering the outer error recovery loop.
    async fn fail_agent(&self, context: &str, error: AgentError) -> Result<(), AgentError> {
        let error_msg = format!("{context}: {error}");
        tracing::error!("failed: {}", error_msg);
        self.emit(Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Stopped {
                reason: StopReason::Failed { error: error_msg },
            },
        }));
        Err(error)
    }

    /// Emit error event (recoverable or not) and log it
    fn emit_error(&self, phase: crate::event::ErrorPhase, error: &str, is_recoverable: bool) {
        if is_recoverable {
            tracing::warn!("{:?} error (recoverable): {}", phase, error);
        } else {
            tracing::error!("{:?} error: {}", phase, error);
        }

        self.emit(Event::Agent(AgentEvent::Error {
            phase,
            error: error.to_string(),
            is_recoverable,
        }));
    }

    /// Emit retrying event
    fn emit_retrying(&self, attempt: u32, max_attempts: u32, reason: &str, wait: Duration) {
        self.emit(Event::Agent(AgentEvent::Retrying {
            attempt,
            max_attempts,
            reason: reason.to_string(),
            wait_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
        }));
    }

    /// Emit operation cancelled event
    fn emit_operation_cancelled(&self, operation: &str) {
        self.emit(Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Stopped {
                reason: StopReason::Cancelled {
                    operation: Some(operation.to_string()),
                },
            },
        }));
    }

    /// Emit `Stopped` lifecycle event with completed reason.
    fn emit_stopped_completed(&self, finish_reason: Option<crate::types::FinishReason>) {
        self.emit(Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Stopped {
                reason: StopReason::Completed { finish_reason },
            },
        }));
    }

    /// Emit user message event to frontend.
    fn emit_user_message_event(
        &self,
        message_id: &crate::types::MessageId,
        content: &[crate::types::ContentBlock],
        is_steer: bool,
    ) {
        // Annotations are model-context decor — the user's own bubble
        // must not show them (nor the host paths they carry).
        let content = crate::utils::asset::strip_image_annotations(content);
        let event = if is_steer {
            crate::event::UserEvent::Steer {
                message_id: message_id.clone(),
                content,
            }
        } else {
            crate::event::UserEvent::Message {
                message_id: message_id.clone(),
                content,
            }
        };
        self.emit(Event::User(event));
    }

    /// Push a message to buffer and emit the storage event (assistant/tool messages).
    fn push_message(&mut self, msg: Message) {
        let msg = Arc::new(msg);
        self.emit(Event::Internal(crate::event::InternalEvent::MessageAdded {
            message: msg.clone(),
        }));
        self.message_buffer.push_arc(msg);
    }

    /// Push a user message: emit frontend event, then push to buffer.
    fn push_user_message(&mut self, msg: Message) {
        let is_steer = msg
            .metadata
            .as_ref()
            .and_then(|meta| meta.get(crate::types::IS_STEER_META_KEY))
            .is_some_and(|value| value == "true");
        self.emit_user_message_event(&msg.id, &msg.content, is_steer);
        self.push_message(msg);
    }

    /// Extract text summary from content blocks for checkpoint display.
    /// Returns first 50 chars of the first text block, handling unicode boundaries.
    /// Replaces newlines with spaces to ensure single-line summary.
    fn extract_summary(content: &[crate::types::ContentBlock]) -> String {
        let text = content
            .iter()
            .find_map(|block| match block {
                crate::types::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or("(no text)");

        // Replace all line endings with spaces to ensure single-line summary
        let text = text.replace(['\n', '\r'], " ");

        // Truncate to 50 chars
        crate::utils::strs::truncate_by_chars(&text, 50, "...")
    }

    /// Start a new turn if not already in one.
    async fn start_turn_if_needed(&mut self) {
        if self.current_turn.is_none() {
            if let Some(msg) = self.message_buffer.messages().iter().rfind(|m| {
                m.role == crate::types::Role::User
                    && !m
                        .metadata
                        .as_ref()
                        .is_some_and(|meta| meta.contains_key(crate::types::INTERRUPTED_META_KEY))
            }) {
                // Extract summary from user message content
                let summary = Self::extract_summary(&msg.content);
                let turn = Arc::new(super::turn::Turn::new(
                    msg.id.clone(),
                    self.session_id.0.clone(),
                    summary,
                    self.checkpoint_store.clone(),
                    &self.data_dir,
                ));
                self.current_turn = Some(turn);
            }
        }
    }

    /// Rewind handler for `AgentInput::Rewind`: cancels any in-flight
    /// turn, then truncates context and files back to the checkpoint.
    async fn process_rewind(
        &mut self,
        message_id: MessageId,
        target: crate::checkpoint::RewindTarget,
        result_tx: tokio::sync::mpsc::Sender<Result<(), AgentError>>,
    ) -> Result<(), AgentError> {
        if let Some(turn) = self.current_turn.take() {
            if let Err(e) = turn.cancel().await {
                tracing::warn!("Failed to cancel turn on rewind: {}", e);
            }
        }

        // Rewind is keyed by message id on two independent stores: the live
        // message buffer (context) and the checkpoint store (files). Either
        // side may be missing — compaction rewrites the buffer but keeps
        // checkpoints, while a message may exist without any checkpoint.
        // Process whichever side has the id; error only when neither does.
        let in_buffer = self
            .message_buffer
            .messages()
            .iter()
            .any(|m| m.id == message_id);

        let has_checkpoint = match self
            .checkpoint_store
            .get_session_checkpoints(&self.session_id)
            .await
        {
            Ok(checkpoints) => checkpoints
                .iter()
                .any(|c| c.message_id == message_id.as_str()),
            Err(e) => {
                // Treat a transient store error as "no checkpoint": context
                // truncation can still proceed, but keep it observable.
                tracing::warn!("Failed to list checkpoints on rewind: {}", e);
                false
            }
        };

        if !in_buffer && !has_checkpoint {
            let err =
                AgentError::Serialization(format!("Message {} not found", message_id.as_str()));
            let _ = result_tx.try_send(Err(err.clone()));
            return Err(err);
        }

        // Checkpoint first: on failure nothing has been mutated yet, so the
        // session stays consistent.
        if has_checkpoint {
            let result = super::turn::Turn::rewind_to_checkpoint(
                &self.session_id,
                &message_id,
                target,
                &self.checkpoint_store,
            )
            .await;

            if let Err(e) = &result {
                let err = AgentError::Serialization(e.to_string());
                let _ = result_tx.try_send(Err(err.clone()));
                return Err(err);
            }
        }

        if in_buffer && target.restore_conversation() {
            let truncated = self.truncate_at(&message_id);
            debug_assert!(truncated, "presence checked above");
        }

        // Always re-emit the authoritative in-memory history: the checkpoint
        // rewind restores an on-disk messages snapshot that may no longer
        // match the buffer (e.g. a pre-compaction snapshot), so the persister
        // must overwrite it with the current buffer.
        let updated_messages: Vec<Arc<Message>> = self.message_buffer.messages().to_vec();
        self.emit(Event::Internal(
            crate::event::InternalEvent::MessageReplaced {
                messages: updated_messages.clone(),
            },
        ));

        if let Err(e) = result_tx.try_send(Ok(())) {
            tracing::warn!("Failed to send rewind success result: {:?}", e);
        }
        Ok(())
    }

    /// Resolve the current provider and model config for this session from the session store.
    /// Caches the result in `current_provider` and `current_model_config`.
    async fn resolve_model(
        &mut self,
    ) -> Result<
        (
            Arc<dyn crate::provider::Provider>,
            Arc<crate::provider::ModelConfig>,
        ),
        AgentError,
    > {
        let (provider, model_config) = self
            .shared
            .resolve_model(&self.session_id)
            .await
            .map_err(|e| AgentError::Other(format!("Model resolution failed: {e}")))?;
        self.current_provider = Some(Arc::clone(&provider));
        self.current_model_config = Some(Arc::clone(&model_config));
        Ok((provider, model_config))
    }

    async fn handle_input(&mut self, input: AgentInput) -> Result<(), AgentError> {
        match input {
            AgentInput::User { content } => {
                self.auto_continue_used = false;
                self.inject_user_message(content, false).await
            }
            AgentInput::Steer(blocks) => self.inject_user_message(blocks, true).await,
            AgentInput::Shutdown => {
                tracing::info!("received close signal");
                if let Some(turn) = self.current_turn.take() {
                    if let Err(e) = turn.cancel().await {
                        tracing::warn!("Failed to cancel turn on shutdown: {}", e);
                    }
                }
                self.emit(Event::Agent(AgentEvent::Lifecycle {
                    state: AgentStatus::Stopped {
                        reason: StopReason::Completed {
                            finish_reason: None,
                        },
                    },
                }));
                Err(AgentError::Shutdown)
            }
            AgentInput::Compact => {
                tracing::info!("received compact request");
                let result = self.force_full_compact().await;
                if let Err(e) = result {
                    tracing::warn!("force_full_compact failed: {}", e);
                }
                Ok(())
            }
            AgentInput::Continue => {
                // Do not reset a pending cancellation: it will be observed by
                // the upcoming streaming select and terminate the loop.
                self.context.transition_to(AgentState::Streaming);
                Ok(())
            }
            AgentInput::Rewind {
                message_id,
                target,
                result_tx,
            } => {
                tracing::info!("received rewind to {}", message_id.as_str());
                self.process_rewind(message_id, target, result_tx).await?;
                Ok(())
            }
            AgentInput::Clear => {
                self.handle_clear().await;
                Ok(())
            }
            AgentInput::PermissionResponse { .. } => {
                // Handled directly by Checker via input_bus subscription
                Ok(())
            }
            AgentInput::AskUserResponse { .. } => {
                // Handled directly by AskUserTool via input_bus subscription
                Ok(())
            }
            AgentInput::Cancel => {
                tracing::info!("received cancel signal");
                self.cancel_token.cancel();
                self.context.transition_to(AgentState::Idle);
                Ok(())
            }
        }
    }

    /// Inject a user message (with interceptors) and transition to Streaming.
    /// Also creates a checkpoint for rewind support.
    async fn inject_user_message(
        &mut self,
        mut content: Vec<ContentBlock>,
        is_steer: bool,
    ) -> Result<(), AgentError> {
        // No cancel reset here: a cancellation that landed after the Idle
        // check must abort the upcoming stream, not be swallowed.
        if let Some(ref interceptor) = self.shared.message_interceptor {
            let ctx = InterceptCtx {
                session_id: &self.session_id,
                history: self.message_buffer.messages(),
            };
            interceptor.intercept(&mut content, &ctx).await;
        }
        let mut msg = Message::with_blocks(Role::User, content);
        if is_steer {
            msg.metadata = Some(std::collections::HashMap::from([(
                crate::types::IS_STEER_META_KEY.to_string(),
                "true".to_string(),
            )]));
        }

        // Note: checkpoint record will be created when turn starts (in start_turn_if_needed)
        // We only persist the message here, the turn object is created later
        self.push_user_message(msg);
        self.context.transition_to(AgentState::Streaming);
        Ok(())
    }

    /// Truncate messages at the given message ID (remove it and everything after).
    /// This rewinds to the state just before this message was sent.
    /// Returns true if truncation was performed, false if message not found.
    fn truncate_at(&mut self, message_id: &MessageId) -> bool {
        let messages = self.message_buffer.messages_mut();

        // Find the index of the target message
        let target_index = messages.iter().position(|m| m.id == *message_id);

        if let Some(index) = target_index {
            // Truncate to keep messages before the target (remove target and everything after)
            messages.truncate(index);

            tracing::info!(
                "Truncated to before message {} (kept {} messages)",
                message_id.as_str(),
                messages.len()
            );
            true
        } else {
            false
        }
    }

    async fn handle_streaming(&mut self) -> Result<(), AgentError> {
        // Resolve current model for this session (cached in self.current_provider / current_model_config)
        let (provider, model_config) = self.resolve_model().await?;

        // 1. Check and run compaction if needed (at the very beginning)
        if self.maybe_compact_messages(&provider, &model_config).await {
            tracing::info!("performed auto-compaction before streaming");
        }

        // 2. Prepare streaming
        let tools = self.tool_registry.definitions();
        tracing::debug!(
            "iteration {}/{}",
            self.context.iteration_count(),
            self.max_iterations,
        );

        let assistant_msg_id = MessageId::new();
        self.emit(Event::Model(ModelEvent::Request {
            message_id: assistant_msg_id.clone(),
            message_count: self.message_buffer.len(),
        }));

        // Validate and clean message buffer before sending to provider
        self.message_buffer.sanitize();

        // Clone messages and tools for the spawned task (needs 'static)
        let messages: Vec<Arc<Message>> = self.message_buffer.messages().to_vec();
        let provider_messages: Vec<Arc<Message>> = messages
            .into_iter()
            .filter(|m| !matches!(m.role, Role::Internal))
            .collect();

        let request_config =
            crate::provider::resolve_request_config(&provider_messages, &tools, &model_config)
                .map_err(AgentError::Provider)?;

        // Spawn provider request in a separate task to allow cancellation
        let stream_task = tokio::spawn(
            async move {
                provider
                    .stream(&provider_messages, &tools, &request_config)
                    .await
            }
            .instrument(tracing::Span::current()),
        );
        let abort_handle = stream_task.abort_handle();

        tracing::debug!("waiting for model stream to start");

        let mut stream = tokio::select! {
            biased;
            () = self.cancel_token.cancelled() => {
                abort_handle.abort();
                return Err(AgentError::Cancelled("stream creation".into()));
            }
            result = stream_task => match result {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => return Err(AgentError::Provider(e)),
                Err(e) if e.is_cancelled() => {
                    return Err(AgentError::Cancelled("stream creation".into()));
                }
                Err(e) => return Err(AgentError::StreamTaskPanicked(e.to_string())),
            }
        };

        let result = self
            .collect_stream_output(&mut stream, assistant_msg_id.clone())
            .await?;

        let end_content = result.content_blocks.clone();

        let has_assistant_result = !result.content_blocks.is_empty()
            || !result.tool_calls.is_empty()
            || result.token_usage.is_some()
            || result.response_id.is_some()
            || result.finish_reason.is_some();
        if has_assistant_result {
            let mut msg = Message::with_blocks(Role::Assistant, result.content_blocks);
            msg.id = assistant_msg_id.clone();
            if !result.tool_calls.is_empty() {
                msg.tool_calls = Some(result.tool_calls);
            }
            if let Some(usage) = result.token_usage {
                msg.token_usage = Some(MessageTokenUsage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens(),
                });
            }
            if let Some(response_id) = result.response_id {
                msg.response_id = Some(response_id);
            }
            msg.model_id = Some(model_config.model_id.clone());
            if let Some(fr) = result.finish_reason {
                msg.finish_reason = Some(fr);
            }

            self.push_message(msg);
        }

        self.emit(Event::Model(ModelEvent::End {
            message_id: assistant_msg_id.clone(),
            content: end_content,
        }));

        self.transition_after_streaming(result.finish_reason).await
    }

    /// Collect all output from the stream until completion
    async fn collect_stream_output(
        &mut self,
        stream: &mut crate::provider::ModelStream,
        message_id: MessageId,
    ) -> Result<super::stream_collector::StreamCollectionResult, AgentError> {
        use super::stream_collector::StreamCollectorState;
        use crate::provider::ModelStreamItem;

        let mut state = StreamCollectorState::default();

        loop {
            tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    return Err(AgentError::Cancelled("streaming".into()));
                }
                item = stream.try_next() => match item {
                    Ok(Some(item)) => match item {
                        ModelStreamItem::Chunk(chunk) => {
                            state.handle_chunk(&chunk);
                            self.emit(Event::Model(ModelEvent::Chunk {
                                message_id: message_id.clone(),
                                content: chunk,
                            }));
                        }
                        ModelStreamItem::ToolCallDelta { id, name, arguments_delta } => {
                            if let Some(summary) =
                                state.handle_tool_call_delta(&id, &name, &arguments_delta)
                            {
                                tracing::warn!("{}", summary);
                            }
                            self.emit(Event::Model(ModelEvent::ToolCallDelta {
                                message_id: message_id.clone(),
                                tool_id: id,
                                tool_name: name,
                                arguments_delta,
                            }));
                        }
                        ModelStreamItem::ToolCall(request) => {
                            state.handle_tool_call(request);
                        }
                        ModelStreamItem::Complete => break,
                        ModelStreamItem::Fallback { from, to } => {
                            self.emit(Event::Model(ModelEvent::Fallback {
                                message_id: message_id.clone(),
                                from,
                                to,
                            }));
                        }
                        ModelStreamItem::TokenUsage(usage) => {
                            let total = usage.total_tokens();
                            state.handle_token_usage(usage);
                            // Context window from the model resolved at stream start;
                            // fall back to the default constant (should not happen).
                            let context_window = self
                                .current_model_config
                                .as_ref()
                                .map_or(crate::compactor::DEFAULT_CONTEXT_WINDOW, |c| {
                                    c.context_window
                                });
                            self.emit(Event::Model(ModelEvent::TokenUsage {
                                message_id: message_id.clone(),
                                prompt_tokens: usage.prompt_tokens,
                                completion_tokens: usage.completion_tokens,
                                total_tokens: total,
                                context_window,
                            }));
                        }
                        ModelStreamItem::ResponseMeta { response_id, finish_reason } => {
                            tracing::debug!(
                                "received response meta: id={:?}, finish_reason={:?}",
                                response_id,
                                finish_reason
                            );
                            state.handle_response_meta(response_id, finish_reason);
                        }
                    },
                    Ok(None) => break,
                    Err(e) => {
                        return Err(AgentError::Provider(e));
                    }
                }
            }
        }

        let result = state.build_result();

        // Record token usage once per stream. Providers may emit TokenUsage
        // multiple times per response (e.g. choice-level and top-level usage
        // chunks); the final event carries the complete values.
        if let (Some(usage), Some(store)) = (result.token_usage, &self.shared.usage_store) {
            if let Some(ref model_config) = self.current_model_config {
                let record = crate::storage::UsageRecord::new(
                    self.session_id.clone(),
                    usage,
                    model_config.model_id.clone(),
                    model_config.provider.to_string(),
                    crate::storage::UsageType::Normal,
                );
                if let Err(e) = store.record(&record).await {
                    tracing::warn!("Failed to record token usage: {}", e);
                }
            }
        }

        Ok(result)
    }

    /// Force full compaction (skip micro-compaction).
    pub async fn force_full_compact(&mut self) -> Result<String, String> {
        // Bracket the whole attempt (model resolution included) so every
        // manual compact — even one that fails up front — emits the event
        // pair status cards materialize and settle on. The agent state
        // stays `Compacting` until the outcome is OUT: a standalone
        // compact's prev state is Idle, and restoring early would let the
        // channel watchdog sweep the live compact card as a false
        // "session lost" timeout while result handling is still awaiting.
        let prev_state = self.begin_compaction();
        let (provider, model_config) = match self.resolve_model().await {
            Ok(resolved) => resolved,
            Err(e) => {
                let error = format!("Model resolution failed: {e}");
                self.emit_compaction_event(false);
                self.emit_compacted(&Err(error.clone()));
                self.restore_compaction_state(prev_state);
                return Err(error);
            }
        };
        let Some(compactor) = self.shared.compactor.as_ref() else {
            let error = "No compactor configured".to_string();
            self.emit_compaction_event(false);
            self.emit_compacted(&Err(error.clone()));
            self.restore_compaction_state(prev_state);
            return Err(error);
        };
        let old_count = self.message_buffer.len();
        let tools = self.tool_registry.definitions();

        let result = compactor
            .full_compact(
                self.message_buffer.messages(),
                &tools,
                provider,
                &model_config,
                Some(self.cancel_token.runtime_token()),
            )
            .await
            .map(Some);

        self.emit_compaction_event(false);
        let outcome = self
            .handle_compaction_result(result, old_count, &model_config)
            .await;
        self.restore_compaction_state(prev_state);
        outcome
    }

    /// Transition into `Compacting` and emit the start event; returns the state
    /// to restore with [`Self::end_compaction`].
    fn begin_compaction(&self) -> AgentState {
        let prev_state = self.context.current_state();
        if !self.context.transition_to(AgentState::Compacting) {
            tracing::warn!("Failed to transition to Compacting from {:?}", prev_state);
        }
        self.emit_compaction_event(true);
        prev_state
    }

    /// Restore the pre-compaction state and emit the end event (auto
    /// compaction mid-run: the restored state is non-Idle, so restoring
    /// before result handling can't trip the channel watchdog).
    fn end_compaction(&self, prev_state: AgentState) {
        self.restore_compaction_state(prev_state);
        self.emit_compaction_event(false);
    }

    /// Restore the pre-compaction state without emitting — the event pair
    /// was already completed.
    fn restore_compaction_state(&self, prev_state: AgentState) {
        if !self.context.transition_to(prev_state) {
            tracing::warn!(
                "Failed to transition back to {:?} from Compacting",
                prev_state
            );
        }
    }

    /// Handle compaction result, persist every rewrite, and clear derived file state.
    async fn handle_compaction_result(
        &mut self,
        result: Result<Option<crate::compactor::CompactionResult>, CompactionError>,
        old_count: usize,
        model_config: &crate::provider::ModelConfig,
    ) -> Result<String, String> {
        let compact_result = match result {
            Ok(None) => Ok("No compaction needed".to_string()),
            Ok(Some(compaction_result)) => {
                // Record compactor token usage
                self.record_compactor_token_usage(compaction_result.token_usage, model_config)
                    .await;

                let rewritten = self
                    .apply_compacted_messages(compaction_result.messages)
                    .await;
                let new_count = self.message_buffer.len();
                let compacted_count = old_count.saturating_sub(new_count);

                if rewritten {
                    if let Some(ref file_state_store) = self.shared.file_state_store {
                        tracing::info!(
                            "clearing file state after conversation compaction ({} -> {} messages)",
                            old_count,
                            new_count
                        );
                        file_state_store.clear().await;
                    }
                }

                Ok(if !rewritten {
                    "No compaction needed".to_string()
                } else if compacted_count > 0 {
                    info!(
                        "compaction completed: {} -> {} messages (compacted {})",
                        old_count, new_count, compacted_count
                    );
                    format!("Compacted {compacted_count} messages")
                } else {
                    "Compaction rewrite completed".to_string()
                })
            }
            Err(CompactionError::Cancelled) => {
                tracing::info!("compaction cancelled");
                self.emit_operation_cancelled("compaction");
                Err("Compaction was cancelled".to_string())
            }
            Err(CompactionError::Api(e) | CompactionError::ContextOverflow(e)) => {
                tracing::warn!("compaction failed: {}", e);
                self.emit_error(crate::event::ErrorPhase::Compaction, &e.clone(), false);
                Err(format!("Compaction failed: {e}"))
            }
        };

        self.emit_compacted(&compact_result);
        compact_result
    }

    /// Emit the compaction outcome — always after the matching
    /// `Compacting { active: false }`, so consumers settle in order.
    fn emit_compacted(&self, result: &Result<String, String>) {
        let (summary, is_error) = match result {
            Ok(summary) => (summary.clone(), false),
            Err(error) => (error.clone(), true),
        };
        self.emit(Event::Model(ModelEvent::Compacted { summary, is_error }));
    }

    /// Emit compaction start/end event.
    fn emit_compaction_event(&self, active: bool) {
        self.emit(Event::Model(ModelEvent::Compacting { active }));
    }

    /// Notify frontends that the mailbox shrank (this agent just consumed
    /// pending items); enqueues/consumes by other paths emit their own.
    async fn emit_mailbox_changed(&self) {
        let (steer, queued) = self.mailbox.lens().await;
        self.emit(Event::Agent(AgentEvent::MailboxChanged { steer, queued }));
    }

    /// Record compactor token usage
    async fn record_compactor_token_usage(
        &self,
        usage: crate::provider::TokenUsage,
        model_config: &crate::provider::ModelConfig,
    ) {
        if usage.prompt_tokens == 0 && usage.completion_tokens == 0 {
            return; // No usage to record
        }
        if let Some(store) = &self.shared.usage_store {
            let record = crate::storage::UsageRecord::new(
                self.session_id.clone(),
                usage,
                model_config.model_id.clone(),
                model_config.provider.to_string(),
                crate::storage::UsageType::Compactor,
            );
            if let Err(e) = store.record(&record).await {
                tracing::warn!("Failed to record compactor token usage: {}", e);
            }
        }
    }

    /// Apply compacted messages and persist every actual history rewrite.
    async fn apply_compacted_messages(&mut self, messages: Vec<Arc<Message>>) -> bool {
        let new_messages: Vec<Arc<Message>> = self
            .message_buffer
            .messages()
            .iter()
            .filter(|message| message.role == Role::System)
            .take(1)
            .cloned()
            .chain(
                messages
                    .iter()
                    .filter(|message| !matches!(message.role, Role::System | Role::Internal))
                    .cloned(),
            )
            .collect();
        if self.message_buffer.messages() == new_messages {
            return false;
        }

        let replacement = new_messages
            .iter()
            .filter(|message| message.role != Role::System)
            .cloned()
            .collect();
        *self.message_buffer.messages_mut() = new_messages;
        self.emit(Event::Internal(
            crate::event::InternalEvent::MessageReplaced {
                messages: replacement,
            },
        ));
        true
    }

    /// Check and run compaction if needed
    /// Returns true if compaction occurred (including full compaction)
    async fn maybe_compact_messages(
        &mut self,
        provider: &Arc<dyn crate::provider::Provider>,
        model_config: &crate::provider::ModelConfig,
    ) -> bool {
        let Some(compactor) = self.shared.compactor.as_ref() else {
            return false; // No compactor configured, skip
        };
        let tools = self.tool_registry.definitions();
        // Pre-check so quiet turns do not flash Compacting state; auto_compact
        // re-evaluates the threshold and then honors the micro-compaction
        // config before falling back to a full summary.
        if !compactor.should_compact(self.message_buffer.messages(), &tools, model_config) {
            return false;
        }
        let old_count = self.message_buffer.len();
        let prev_state = self.begin_compaction();
        let result = compactor
            .auto_compact(
                self.message_buffer.messages(),
                &tools,
                Arc::clone(provider),
                model_config,
                Some(self.cancel_token.runtime_token()),
            )
            .await;
        self.end_compaction(prev_state);
        match self
            .handle_compaction_result(result, old_count, model_config)
            .await
        {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("auto-compaction failed: {}", e);
                false
            }
        }
    }

    /// Transition to appropriate state after streaming completes
    async fn transition_after_streaming(
        &mut self,
        finish_reason: Option<crate::types::FinishReason>,
    ) -> Result<(), AgentError> {
        let has_tool_calls = self
            .message_buffer
            .messages()
            .last()
            .and_then(|m| m.tool_calls.as_ref())
            .is_some();

        let is_consistent = is_stream_completion_consistent(finish_reason, has_tool_calls);

        if !is_consistent {
            let error = format!(
                "inconsistent model stream completion: finish_reason={finish_reason:?}, has_tool_calls={has_tool_calls}"
            );
            tracing::error!("{error}");
            self.emit(Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Stopped {
                    reason: StopReason::Failed {
                        error: error.clone(),
                    },
                },
            }));
            self.context.transition_to(AgentState::Idle);
            return Ok(());
        }

        match finish_reason {
            Some(FinishReason::ToolCalls) => {
                let tool_count = self
                    .message_buffer
                    .messages()
                    .last()
                    .and_then(|m| m.tool_calls.as_ref())
                    .map_or(0, |v| v.len());
                tracing::debug!(
                    "detected {} tool call(s), transitioning to ExecutingTool",
                    tool_count
                );
                self.context.transition_to(AgentState::ExecutingTool);
                return Ok(());
            }
            Some(FinishReason::MaxTokens) => {
                if should_auto_continue(&mut self.auto_continue_used, finish_reason) {
                    tracing::info!(?finish_reason, "auto-injecting 'continue' user message");
                    let msg = Message::user("continue");
                    self.push_user_message(msg);
                    self.context.transition_to(AgentState::Streaming);
                } else {
                    tracing::warn!(
                        ?finish_reason,
                        "model stopped again after auto-continue; not continuing a second time"
                    );
                    self.emit_stopped_completed(finish_reason);
                    self.context.transition_to(AgentState::Idle);
                }
                return Ok(());
            }
            Some(FinishReason::PauseTurn) => {
                let error = "Anthropic pause_turn requires preserving server-side tool state, which is not supported";
                tracing::error!("{error}");
                self.emit(Event::Agent(AgentEvent::Lifecycle {
                    state: AgentStatus::Stopped {
                        reason: StopReason::Failed {
                            error: error.to_string(),
                        },
                    },
                }));
                self.context.transition_to(AgentState::Idle);
                return Ok(());
            }
            Some(FinishReason::Refusal) => {
                self.emit_stopped_completed(finish_reason);
                self.context.transition_to(AgentState::Idle);
                return Ok(());
            }
            Some(FinishReason::Stop | FinishReason::Repeat) => {}
            None | Some(FinishReason::ContentFilter | FinishReason::Unknown) => {
                unreachable!("inconsistent finish reasons returned above")
            }
        }

        self.emit_stopped_completed(finish_reason);
        self.context.transition_to(AgentState::Idle);
        Ok(())
    }

    // handle_execute_tool is defined in tool_exec.rs

    #[tracing::instrument(skip(self))]
    async fn handle_streaming_with_retry(&mut self) -> Result<(), AgentError> {
        // Loose budget for rate limits (429): exponential backoff with
        // jitter, `Retry-After` honored. The counter is per turn — a
        // successful stream returns Ok and the next turn starts fresh.
        let max_retries = crate::utils::env::env_parse::<u32>(&format!(
            "{}STREAM_MAX_RETRIES",
            crate::ENV_PREFIX
        ))
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_STREAM_MAX_RETRIES);
        let mut attempt = 0;
        let mut context_recovery_attempted = false;

        loop {
            match self.handle_streaming().await {
                Ok(()) => return Ok(()),
                Err(e) if e.is_cancelled() => return Err(e),
                Err(e) if e.is_context_overflow() && !context_recovery_attempted => {
                    context_recovery_attempted = true;
                    tracing::warn!(
                        "streaming input exceeded the provider context window; forcing compaction"
                    );
                    if let Err(compaction_error) = self.force_full_compact().await {
                        return self
                            .fail_agent(
                                "Context overflow recovery compaction failed",
                                AgentError::Other(compaction_error),
                            )
                            .await;
                    }
                    tracing::info!("context overflow recovery compaction completed; retrying");
                }
                Err(e) if e.is_context_overflow() => {
                    return self
                        .fail_agent("Context overflow persisted after compaction", e)
                        .await;
                }
                Err(e) if attempt >= max_retries => {
                    return self
                        .fail_agent("Streaming failed after max retries", e)
                        .await;
                }
                Err(e) if !should_retry_streaming_error(attempt, e.is_retryable()) => {
                    return self
                        .fail_agent("Streaming failed with non-retryable error", e)
                        .await;
                }
                Err(e) => {
                    attempt += 1;
                    let delay = retry_delay(attempt, e.retry_after());
                    tracing::warn!(
                        "Streaming failed (attempt {}/{}), retrying in {:?}: {}",
                        attempt,
                        max_retries,
                        delay,
                        e
                    );
                    // Error first, Retrying second: the status card keeps a
                    // single phase slot, and the Retrying phase (attempt
                    // count + delay + reason) is strictly more informative
                    // than the bare error line it would otherwise be
                    // overwritten by.
                    self.emit_error(crate::event::ErrorPhase::Streaming, &e.to_string(), true);
                    self.emit_retrying(attempt, max_retries, &e.to_string(), delay);
                    wait_for_retry(&self.cancel_token, delay).await?;
                }
            }
        }
    }
}

fn should_retry_streaming_error(attempt: u32, retryable: bool) -> bool {
    retryable || attempt == 0
}

/// Default streaming retry budget per turn (`YOMI_STREAM_MAX_RETRIES` overrides).
const DEFAULT_STREAM_MAX_RETRIES: u32 = 20;
/// Exponential backoff for retries without a server hint: 1s × 2^(n-1), capped.
const RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
const RETRY_MAX_DELAY: Duration = Duration::from_mins(5);
/// Cap for server-provided `Retry-After` hints — window-based rate limits
/// (per-minute/hour quotas) legitimately ask for long waits.
const RETRY_AFTER_CAP: Duration = Duration::from_mins(30);
/// Floor for server-provided hints — `Retry-After: 0` must not spin a hot
/// retry loop.
const RETRY_AFTER_FLOOR: Duration = Duration::from_secs(1);

/// Delay before the next streaming retry (claude-code `withRetry.ts`
/// shape): a server `Retry-After` hint wins (clamped to [1s, 30min]);
/// otherwise exponential backoff. Both carry +0–25% jitter to avoid a
/// retry herd.
fn retry_delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    use rand::RngExt;

    let base = if let Some(hint) = retry_after {
        hint.clamp(RETRY_AFTER_FLOOR, RETRY_AFTER_CAP)
    } else {
        let shift = attempt.saturating_sub(1).min(20);
        RETRY_BASE_DELAY
            .saturating_mul(1u32 << shift)
            .min(RETRY_MAX_DELAY)
    };
    let jitter_ms = rand::rng().random_range(0..=base.as_millis() as u64 / 4);
    (base + Duration::from_millis(jitter_ms)).min(RETRY_AFTER_CAP)
}

fn is_stream_completion_consistent(
    finish_reason: Option<FinishReason>,
    has_tool_calls: bool,
) -> bool {
    match finish_reason {
        Some(FinishReason::ToolCalls) => has_tool_calls,
        Some(
            FinishReason::Stop
            | FinishReason::MaxTokens
            | FinishReason::PauseTurn
            | FinishReason::Refusal
            | FinishReason::Repeat,
        ) => !has_tool_calls,
        None | Some(FinishReason::ContentFilter | FinishReason::Unknown) => false,
    }
}

fn should_auto_continue(used: &mut bool, finish_reason: Option<FinishReason>) -> bool {
    if *used || finish_reason != Some(FinishReason::MaxTokens) {
        return false;
    }
    *used = true;
    true
}

async fn wait_for_retry(cancel_token: &CancelToken, delay: Duration) -> Result<(), AgentError> {
    tokio::select! {
        biased;
        () = cancel_token.cancelled() => {
            Err(AgentError::Cancelled("streaming retry".into()))
        }
        () = tokio::time::sleep(delay) => Ok(()),
    }
}

#[cfg(test)]
#[path = "agent_test.rs"]
mod tests;
