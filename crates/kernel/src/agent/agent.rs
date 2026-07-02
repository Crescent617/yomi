use super::message_buffer::MessageBuffer;
use super::{
    AgentError, AgentExecutionContext, AgentHandle, AgentShared, AgentSpawnArgs, AgentState,
    CancelToken, InterceptCtx,
};
use crate::compactor::{CompactionError, DEFAULT_CONTEXT_WINDOW};
use crate::event::{
    AgentEvent, AgentStatus, Event, ModelEvent, StopReason, SystemEvent, ToolEvent,
};
use crate::event_bus::EventBusHandle;
use crate::permissions::Checker;
use crate::prompt::SystemPromptBuilder;
use crate::tools::executor::{ToolExecParams, ToolExecutionResult};
use crate::types::{AgentId, ContentBlock, Message, MessageId, MessageTokenUsage, Role, SessionId};
use crate::FinishReason;
use futures::TryStreamExt;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, info_span, warn, Instrument};

/// Input messages that can be sent to an Agent
pub enum AgentInput {
    /// User message with multi-modal content blocks
    User {
        content: Vec<ContentBlock>,
        /// Generation counter at send time; lower values are stale (cancelled before send)
        generation: u64,
    },
    /// Continue the agent from Idle to Streaming (used by goal auto-start)
    Continue,
    /// Background task completion
    TaskResult {
        task_id: String,
        content: Vec<ContentBlock>,
    },
    /// Permission response from user/TUI
    PermissionResponse { req_id: String, approved: bool },
    /// Shutdown the agent gracefully (for subagent/resource management)
    Shutdown,
    /// Force compaction of message buffer
    Compact,
    /// Rewind to a specific checkpoint
    Rewind {
        message_id: MessageId,
        target: crate::checkpoint::RewindTarget,
        /// Channel to send the result back
        result_tx: tokio::sync::oneshot::Sender<Result<(), AgentError>>,
    },
    /// Clear the agent's context (messages, file state, todos, persisted history)
    Clear,
}

pub struct Agent {
    id: AgentId,
    shared: Arc<AgentShared>,
    message_buffer: MessageBuffer,
    event_bus: EventBusHandle,
    input_rx: mpsc::Receiver<AgentInput>,
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
    /// Generation counter: inputs with lower generation are stale (cancelled before send)
    input_stale_since: Arc<AtomicU64>,
    /// Hook registry for `PreToolUse` / `PostToolUse` / `PreStop` lifecycle hooks
    hook_registry: crate::hooks::HookRegistry,
    /// Checkpoint store for persistence
    checkpoint_store: Arc<dyn crate::checkpoint::CheckpointStore>,
    /// Data directory for checkpoints
    data_dir: std::path::PathBuf,
    /// Current turn (contains tracked files, shared with tools)
    current_turn: Option<Arc<super::turn::Turn>>,
    /// Current skills list (available to tools)
    skills: Vec<Arc<crate::skill::Skill>>,
    /// Channel for receiving steer messages injected before each streaming turn
    steer_rx: mpsc::Receiver<Vec<ContentBlock>>,
    /// Maximum tool output length in bytes
    max_tool_output_length: usize,
}

impl Agent {
    pub async fn spawn(
        id: AgentId,
        shared: &Arc<AgentShared>,
        args: AgentSpawnArgs,
    ) -> AgentHandle {
        let (input_tx, input_rx) = mpsc::channel::<AgentInput>(20);
        let (steer_tx, steer_rx) = mpsc::channel::<Vec<ContentBlock>>(20);
        let cancel_token = args.cancel_token.clone().unwrap_or_default();
        let (context, state_rx) = AgentExecutionContext::new(AgentState::Idle);

        // Get event bus handle from shared
        let session_id = SessionId(args.session_id.clone());
        let event_bus = shared
            .event_bus
            .as_ref()
            .expect("event_bus must be configured")
            .handle(session_id.clone());

        // Build system prompt with project memory and skills
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

        // Create ask-user state (session-level, independent of permission state)
        let (ask_user_state, ask_user_responder) = crate::tools::AskUserState::new();

        // Create agent-specific tool registry with standard tools
        let tool_registry = crate::tools::ToolRegistryFactory::create(
            crate::tools::ToolRegistryConfig::for_main_agent(
                &id,
                &shared,
                &input_tx,
                &event_bus,
                &args.session_id,
                args.tool_blocklist.clone(),
            )
            .with_enable_subagent(args.enable_subagent)
            .with_file_state_store(args.file_state_store.clone())
            .with_ask_user_state(ask_user_state.clone()),
        );

        // Build hook registry: if user-level hooks are enabled (Some), also load
        // skill-level hooks. When hooks are disabled (None) the registry stays
        // empty and `run_*_tool_hooks` will short-circuit without spawning.
        let hook_registry = crate::hooks::build_hook_registry_with_skills(
            shared.hook_registry.as_deref(),
            &args.skills,
            args.allow_command_hooks,
            shared.goal_store.clone(),
        )
        .await;

        // Create permission checker and responder from shared state
        // If no permission_state in shared (YOLO mode), all tools auto-approve
        let (permission_checker, permission_responder) = match shared.permission_state.as_ref() {
            Some(state) => {
                let checker = Checker::new(state.clone(), id.clone(), event_bus.clone());
                let responder = state.create_responder();
                (Some(Arc::new(checker)), Some(responder))
            }
            None => (None, None),
        };

        // Shared generation counter for tracking stale inputs (incremented on cancel)
        let input_stale_since: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));

        // Get checkpoint store
        let checkpoint_store = shared.checkpoint_store.clone().unwrap_or_else(|| {
            Arc::new(crate::checkpoint::FilesystemCheckpointStore::new(
                &shared.data_dir,
            ))
        });

        let data_dir = shared.data_dir.clone();

        let agent = Self {
            id: id.clone(),
            shared,
            message_buffer,
            event_bus: event_bus.clone(),
            input_rx,
            context,
            cancel_token: cancel_token.clone(),
            session_id: session_id.clone(),
            max_iterations: args.max_iterations,
            tool_registry,
            permission_checker,
            working_dir: args.working_dir,
            input_stale_since: Arc::clone(&input_stale_since),
            hook_registry,
            checkpoint_store,
            data_dir,
            current_turn: None,
            skills: args.skills.clone(),
            steer_rx,
            max_tool_output_length: args.max_tool_output_length,
        };

        let span = info_span!("agent", session_id = %session_id.0);

        tokio::spawn(
            async move {
                let result = agent.start_loop().await;
                if let Err(ref e) = result {
                    tracing::error!("Agent failed: {}", e);
                }
                // Explicitly send shutdown event
                let _ = event_bus.try_send(Event::System(SystemEvent::Shutdown {
                    session_id: session_id.clone(),
                    error: None,
                }));
                info!("agent closed");
            }
            .instrument(span),
        );

        AgentHandle::new(
            id,
            input_tx,
            state_rx,
            cancel_token,
            permission_responder,
            Some(ask_user_responder),
            Arc::clone(&input_stale_since),
            steer_tx,
        )
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
        if let Some(ref store) = self.shared.message_store {
            let to_persist: Vec<Message> = self
                .message_buffer
                .messages()
                .iter()
                .map(|m| (**m).clone())
                .collect();
            if let Err(e) = store.replace(&self.session_id.0, &to_persist).await {
                tracing::warn!("failed to clear persisted messages: {}", e);
            }
        }
    }

    /// Persist a single message to storage
    async fn persist_message(&self, message: &Message) {
        if let Some(store) = &self.shared.message_store {
            if let Err(e) = store
                .append(&self.session_id, std::slice::from_ref(message))
                .await
            {
                tracing::warn!("Failed to persist message: {}", e);
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn start_loop(mut self) -> Result<(), AgentError> {
        tracing::info!(
            "started with {} initial message(s), max_iterations={}",
            self.message_buffer.len(),
            self.max_iterations
        );

        loop {
            let state = self.context.current_state();

            if state.is_terminal() {
                break;
            }

            if self.max_iterations > 0
                && self.context.iteration_count() >= self.max_iterations
                && self.context.current_state() == AgentState::Streaming
            {
                let skip_max_iterations = match &self.shared.goal_store {
                    Some(store) => match store.load(&self.session_id).await {
                        Ok(Some(goal)) => matches!(goal.status, crate::goal::GoalStatus::Active),
                        _ => false,
                    },
                    None => false,
                };
                if !skip_max_iterations {
                    tracing::warn!(
                        "reached max iterations during streaming, cancelling and returning to waiting for input"
                    );
                    // Notify TUI that max iterations reached
                    if let Err(e) = self.event_bus.try_send(Event::Agent(AgentEvent::Lifecycle {
                        agent_id: self.id.clone(),
                        state: AgentStatus::Stopped {
                            reason: StopReason::MaxIterations {
                                reached: self.max_iterations,
                            },
                        },
                    })) {
                        tracing::warn!("Failed to send max iterations event: {}", e);
                    }
                    self.context.transition_to(AgentState::Idle);
                    continue;
                }
            }

            // Note: cancel is handled during streaming via select!, not here
            let result = match state {
                AgentState::Idle => {
                    self.context.reset_iteration();
                    self.handle_idle().await
                }
                AgentState::Streaming => {
                    // Start new turn when entering Streaming
                    self.start_turn_if_needed().await;
                    tracing::debug!("starting streaming");
                    // Notify UI that streaming has started
                    if let Err(e) = self.event_bus.try_send(Event::Agent(AgentEvent::Lifecycle {
                        agent_id: self.id.clone(),
                        state: AgentStatus::Running,
                    })) {
                        tracing::warn!("Failed to send streaming start event: {}", e);
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
                AgentState::Closed => break,
            };

            // Handle state transition after execution
            if let Err(e) = result {
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
                        AgentState::Closed => unreachable!(),
                    };
                    self.emit_error(phase, &e.to_string(), false).await;

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
    }

    /// Handle cancellation - sends Cancelled event, transitions state, returns Ok(())
    async fn handle_cancel(&self, context: &str) -> Result<(), AgentError> {
        tracing::info!("{} cancelled", context);
        // Emit cancellation event with operation name
        self.emit_operation_cancelled(context).await;
        self.context.transition_to(AgentState::Idle);
        Ok(())
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

    /// Helper to emit `AgentEvent::Lifecycle(Stopped(Failed))` and return error
    async fn fail_agent(&self, context: &str, error: AgentError) -> Result<(), AgentError> {
        let error_msg = format!("{context}: {error}");
        tracing::error!("failed: {}", error_msg);
        let _n = self
            .event_bus
            .send(Event::Agent(AgentEvent::Lifecycle {
                agent_id: self.id.clone(),
                state: AgentStatus::Stopped {
                    reason: StopReason::Failed { error: error_msg },
                },
            }))
            .await;
        Err(error)
    }

    /// Emit error event (recoverable or not) and log it
    async fn emit_error(&self, phase: crate::event::ErrorPhase, error: &str, is_recoverable: bool) {
        if is_recoverable {
            tracing::warn!("{:?} error (recoverable): {}", phase, error);
        } else {
            tracing::error!("{:?} error: {}", phase, error);
        }

        if let Err(e) = self.event_bus.try_send(Event::Agent(AgentEvent::Error {
            agent_id: self.id.clone(),
            phase,
            error: error.to_string(),
            is_recoverable,
        })) {
            tracing::warn!("Failed to emit error event: {}", e);
        }
    }

    /// Emit retrying event
    async fn emit_retrying(&self, attempt: u32, max_attempts: u32, reason: &str) {
        if let Err(e) = self.event_bus.try_send(Event::Agent(AgentEvent::Retrying {
            agent_id: self.id.clone(),
            attempt,
            max_attempts,
            reason: reason.to_string(),
        })) {
            tracing::warn!("Failed to emit retrying event: {}", e);
        }
    }

    /// Emit operation cancelled event
    async fn emit_operation_cancelled(&self, operation: &str) {
        if let Err(e) = self.event_bus.try_send(Event::Agent(AgentEvent::Lifecycle {
            agent_id: self.id.clone(),
            state: AgentStatus::Stopped {
                reason: StopReason::Cancelled {
                    operation: Some(operation.to_string()),
                },
            },
        })) {
            tracing::warn!("Failed to emit operation cancelled event: {}", e);
        }
    }

    /// Emit `Stopped` lifecycle event with completed reason.
    fn emit_stopped_completed(&self, finish_reason: Option<crate::types::FinishReason>) {
        if let Err(e) = self.event_bus.try_send(Event::Agent(AgentEvent::Lifecycle {
            agent_id: self.id.clone(),
            state: AgentStatus::Stopped {
                reason: StopReason::Completed { finish_reason },
            },
        })) {
            tracing::warn!("Failed to send Stopped::Completed event: {}", e);
        }
    }

    /// Emit user message event to frontend.
    fn emit_user_message_event(
        &self,
        message_id: &crate::types::MessageId,
        content: &[crate::types::ContentBlock],
    ) {
        if let Err(e) = self
            .event_bus
            .try_send(Event::User(crate::event::UserEvent::Message {
                message_id: message_id.clone(),
                content: content.to_vec(),
            }))
        {
            tracing::warn!("Failed to send user message event: {}", e);
        }
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

        // Replace newlines with spaces to ensure single-line summary
        let text = text.replace('\n', " ");

        // Truncate to 50 chars
        crate::utils::strs::truncate_by_chars(&text, 50, "...")
    }

    /// Start a new turn if not already in one.
    async fn start_turn_if_needed(&mut self) {
        if self.current_turn.is_none() {
            if let Some(msg) = self
                .message_buffer
                .messages()
                .iter()
                .rfind(|m| m.role == crate::types::Role::User)
            {
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

    /// Shared rewind handler used by both normal input loop and goal idle.
    #[tracing::instrument(skip(self))]
    async fn process_rewind(
        &mut self,
        message_id: MessageId,
        target: crate::checkpoint::RewindTarget,
        result_tx: tokio::sync::oneshot::Sender<Result<(), AgentError>>,
    ) -> Result<(), AgentError> {
        if let Some(turn) = self.current_turn.take() {
            if let Err(e) = turn.cancel().await {
                tracing::warn!("Failed to cancel turn on rewind: {}", e);
            }
        }

        let truncated = self.truncate_at(&message_id);
        if !truncated {
            let err =
                AgentError::Serialization(format!("Message {} not found", message_id.as_str()));
            let _ = result_tx.send(Err(err.clone()));
            return Err(err);
        }

        let result = super::turn::Turn::rewind_to_checkpoint(
            &self.session_id,
            &message_id,
            target,
            &self.checkpoint_store,
        )
        .await;

        if let Err(e) = &result {
            let err = AgentError::Serialization(e.to_string());
            let _ = result_tx.send(Err(err.clone()));
            return Err(err);
        }

        let remaining_messages: Vec<Message> = self
            .message_buffer
            .messages()
            .iter()
            .map(|m| (**m).clone())
            .collect();
        if let Some(msg_store) = &self.shared.message_store {
            if let Err(e) = msg_store
                .replace(&self.session_id, &remaining_messages)
                .await
            {
                tracing::warn!("Failed to update message store after rewind: {}", e);
            }
        }

        let updated_messages: Vec<Arc<Message>> = self.message_buffer.messages().to_vec();
        if let Err(e) = self
            .event_bus
            .try_send(Event::System(crate::event::SystemEvent::Rewound {
                session_id: self.session_id.clone(),
                messages: updated_messages,
            }))
        {
            tracing::warn!("Failed to send rewound event: {}", e);
        }

        if let Err(e) = result_tx.send(Ok(())) {
            tracing::warn!("Failed to send rewind success result: {:?}", e);
        }
        Ok(())
    }

    /// Unified idle handler.
    ///
    /// Drains pending external inputs. If an active goal exists and no user input
    /// is pending, auto-continue by injecting the goal continuation prompt.
    /// Handle idle state: block until a user input arrives.
    #[tracing::instrument(skip(self))]
    async fn handle_idle(&mut self) -> Result<(), AgentError> {
        if self.cancel_token.is_cancelled() {
            self.cancel_token.reset_if_cancelled();
            return Ok(());
        }

        let input = tokio::select! {
            biased;
            input = self.input_rx.recv() => input,
            steer = self.steer_rx.recv() => {
                steer.map(|blocks| AgentInput::User {
                    content: blocks,
                    generation: self.input_stale_since.load(Ordering::Relaxed),
                })
            }
        };

        match input {
            Some(AgentInput::User {
                content,
                generation,
            }) => {
                let current_gen = self.input_stale_since.load(Ordering::Relaxed);
                if generation < current_gen {
                    tracing::info!(
                        "discarding stale user input (generation {} < {})",
                        generation,
                        current_gen
                    );
                    return Ok(());
                }
                self.inject_user_message(content).await
            }
            Some(AgentInput::TaskResult { task_id, content }) => {
                tracing::debug!("Task result received: {}", task_id);
                self.cancel_token.reset_if_cancelled();
                let msg = Message::with_blocks(Role::User, content);
                self.persist_message(&msg).await;
                self.message_buffer.push(msg);
                self.context.transition_to(AgentState::Streaming);
                Ok(())
            }
            Some(AgentInput::PermissionResponse {
                req_id,
                approved: _,
            }) => {
                tracing::warn!("received PermissionResponse via input channel (should use PermissionResponder instead): req_id={}", req_id);
                Ok(())
            }
            Some(AgentInput::Shutdown) => {
                tracing::info!("received close signal");
                if let Some(turn) = self.current_turn.take() {
                    if let Err(e) = turn.cancel().await {
                        tracing::warn!("Failed to cancel turn on shutdown: {}", e);
                    }
                }
                self.context.transition_to(AgentState::Closed);
                Ok(())
            }
            Some(AgentInput::Compact) => {
                tracing::info!("received compact request");
                let result = self.force_full_compact().await;
                if let Err(e) = result {
                    tracing::warn!("force_full_compact failed: {}", e);
                }
                Ok(())
            }
            Some(AgentInput::Continue) => {
                self.cancel_token.reset_if_cancelled();
                self.context.transition_to(AgentState::Streaming);
                Ok(())
            }
            Some(AgentInput::Rewind {
                message_id,
                target,
                result_tx,
            }) => {
                tracing::info!("received rewind to {}", message_id.as_str());
                self.process_rewind(message_id, target, result_tx).await?;
                Ok(())
            }
            Some(AgentInput::Clear) => {
                self.handle_clear().await;
                Ok(())
            }
            None => {
                tracing::info!("input channel closed (clean shutdown)");
                self.context.transition_to(AgentState::Closed);
                Ok(())
            }
        }
    }

    /// Inject a user message (with interceptors) and transition to Streaming.
    /// Also creates a checkpoint for rewind support.
    #[tracing::instrument(skip(self))]
    async fn inject_user_message(
        &mut self,
        mut content: Vec<ContentBlock>,
    ) -> Result<(), AgentError> {
        self.cancel_token.reset_if_cancelled();
        if let Some(ref interceptor) = self.shared.message_interceptor {
            let ctx = InterceptCtx {
                session_id: &self.session_id,
                history: self.message_buffer.messages(),
            };
            interceptor.intercept(&mut content, &ctx).await;
        }
        let msg = Message::with_blocks(Role::User, content);

        // Note: checkpoint record will be created when turn starts (in start_turn_if_needed)
        // We only persist the message here, the turn object is created later
        self.emit_user_message_event(&msg.id, &msg.content);
        self.persist_message(&msg).await;
        self.message_buffer.push(msg);
        self.context.transition_to(AgentState::Streaming);
        Ok(())
    }

    /// Truncate messages at the given message ID (remove it and everything after).
    /// This rewinds to the state just before this message was sent.
    /// Returns true if truncation was performed, false if message not found.
    #[tracing::instrument(skip(self))]
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

    #[tracing::instrument(skip(self))]
    async fn handle_streaming(&mut self) -> Result<(), AgentError> {
        // 1. Check and run compaction if needed (at the very beginning)
        if self.maybe_compact_messages().await {
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
        if let Err(e) = self.event_bus.try_send(Event::Model(ModelEvent::Request {
            agent_id: self.id.clone(),
            message_id: assistant_msg_id.clone(),
            message_count: self.message_buffer.len(),
        })) {
            tracing::warn!("Failed to send model request event: {}", e);
        }

        // Drain pending steer messages and inject them as a user message before streaming
        let mut steer_blocks: Vec<ContentBlock> = Vec::new();
        while let Ok(blocks) = self.steer_rx.try_recv() {
            steer_blocks.extend(blocks);
        }
        if !steer_blocks.is_empty() {
            tracing::info!(
                "injecting {} steer block(s) before streaming",
                steer_blocks.len()
            );
            let steer_msg = Message::with_blocks(Role::User, steer_blocks);
            self.emit_user_message_event(&steer_msg.id, &steer_msg.content);
            self.persist_message(&steer_msg).await;
            self.message_buffer.push(steer_msg);
        }

        // Validate and clean message buffer before sending to provider
        self.message_buffer.sanitize();

        // Clone messages and tools for the spawned task (needs 'static)
        let messages: Vec<Arc<Message>> = self.message_buffer.messages().to_vec();
        let messages =
            crate::utils::asset::resolve_messages(&messages, &self.shared.data_dir).await;

        // Spawn provider request in a separate task to allow cancellation
        let provider = self.shared.provider.clone();
        let model_config = self.shared.model_config.clone();
        let stream_task = tokio::spawn(
            async move { provider.stream(&messages, &tools, &model_config).await }
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

        if !result.content_blocks.is_empty() || !result.tool_calls.is_empty() {
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
            if let Some(fr) = result.finish_reason {
                msg.finish_reason = Some(fr);
            }

            self.persist_message(&msg).await;
            self.message_buffer.push(msg);
        }

        if let Err(e) = self.event_bus.try_send(Event::Model(ModelEvent::Completed {
            agent_id: self.id.clone(),
            message_id: assistant_msg_id.clone(),
        })) {
            tracing::warn!("Failed to send completed event: {}", e);
        }

        if let Err(e) = self.event_bus.try_send(Event::Model(ModelEvent::End {
            agent_id: self.id.clone(),
            message_id: assistant_msg_id.clone(),
            content: end_content,
        })) {
            tracing::warn!("Failed to send model end event: {}", e);
        }

        if result.finish_reason.is_none() {
            tracing::warn!("model response has no finish_reason");
            self.emit_error(
                crate::event::ErrorPhase::Streaming,
                "model response missing finish_reason",
                true,
            )
            .await;
        }
        self.transition_after_streaming(result.finish_reason).await
    }

    /// Collect all output from the stream until completion
    #[tracing::instrument(skip(self, stream))]
    async fn collect_stream_output(
        &mut self,
        stream: &mut crate::providers::ModelStream,
        message_id: MessageId,
    ) -> Result<super::stream_collector::StreamCollectionResult, AgentError> {
        use super::stream_collector::StreamCollectorState;
        use crate::providers::ModelStreamItem;

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
                            if let Err(e) = self.event_bus.try_send(Event::Model(ModelEvent::Chunk {
                                agent_id: self.id.clone(),
                                message_id: message_id.clone(),
                                content: chunk,
                            })) {
                                tracing::warn!("Failed to send chunk event: {}", e);
                            }
                        }
                        ModelStreamItem::ToolCallDelta { id, name, arguments_delta } => {
                            // Forward incremental tool call update to TUI for UI feedback
                            if let Err(e) = self.event_bus.try_send(Event::Model(ModelEvent::ToolCallDelta {
                                agent_id: self.id.clone(),
                                message_id: message_id.clone(),
                                tool_id: id,
                                tool_name: name,
                                arguments_delta,
                            })) {
                                tracing::warn!("Failed to send tool call delta event: {}", e);
                            }
                        }
                        ModelStreamItem::ToolCall(request) => {
                            state.handle_tool_call(request);
                        }
                        ModelStreamItem::Complete => break,
                        ModelStreamItem::Fallback { from, to } => {
                            if let Err(e) = self.event_bus.try_send(Event::Model(ModelEvent::Fallback {
                                agent_id: self.id.clone(),
                                message_id: message_id.clone(),
                                from,
                                to,
                            })) {
                                tracing::warn!("Failed to send fallback event: {}", e);
                            }
                        }
                        ModelStreamItem::TokenUsage(usage) => {
                            // NOTE: this is right because each response's prompt_tokens will contain whole history
                            tracing::info!(
                                "received token usage update: prompt={}, completion={}, total={}",
                                usage.prompt_tokens,
                                usage.completion_tokens,
                                usage.total_tokens()
                            );
                            let total = usage.total_tokens();
                            state.handle_token_usage(usage);
                            // Get context window from compactor or use default
                            let context_window = self.shared.compactor.as_ref()
                                .map_or(DEFAULT_CONTEXT_WINDOW, |c| c.context_window);
                            if let Err(e) = self.event_bus.try_send(Event::Model(ModelEvent::TokenUsage {
                                agent_id: self.id.clone(),
                                message_id: message_id.clone(),
                                prompt_tokens: usage.prompt_tokens,
                                completion_tokens: usage.completion_tokens,
                                total_tokens: total,
                                context_window,
                            })) {
                                tracing::warn!("Failed to send token usage event: {}", e);
                            }
                            // Record token usage
                            if let Some(store) = &self.shared.usage_store {
                                let record = crate::storage::UsageRecord::new(
                                    self.session_id.clone(),
                                    self.id.clone(),
                                    usage,
                                    self.shared.model_config.model_id.clone(),
                                    self.shared.model_config.provider.to_string(),
                                    crate::storage::UsageType::Normal,
                                );
                                if let Err(e) = store.record(&record).await {
                                    tracing::warn!("Failed to record token usage: {}", e);
                                }
                            }
                        }
                        ModelStreamItem::ResponseMeta { response_id, finish_reason } => {
                            tracing::debug!(
                                "received response meta: id={}, finish_reason={:?}",
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

        Ok(state.build_result())
    }

    /// Force compaction regardless of threshold.
    #[tracing::instrument(skip(self))]
    pub async fn force_compact(&mut self) -> Result<String, String> {
        let compactor = self
            .shared
            .compactor
            .as_ref()
            .ok_or("No compactor configured")?;
        let old_count = self.message_buffer.len();
        let prev_state = self.context.current_state();
        if !self.context.transition_to(AgentState::Compacting) {
            tracing::warn!("Failed to transition to Compacting from {:?}", prev_state);
        }
        self.emit_compaction_event(true).await;

        let result = compactor
            .auto_compact(
                self.message_buffer.messages(),
                Arc::clone(&self.shared.provider),
                &self.shared.model_config,
                Some(self.cancel_token.runtime_token()),
            )
            .await;

        if !self.context.transition_to(prev_state) {
            tracing::warn!(
                "Failed to transition back to {:?} from Compacting",
                prev_state
            );
        }
        self.emit_compaction_event(false).await;
        self.handle_compaction_result(result, old_count).await
    }

    /// Force full compaction (skip micro-compaction).
    #[tracing::instrument(skip(self))]
    pub async fn force_full_compact(&mut self) -> Result<String, String> {
        let compactor = self
            .shared
            .compactor
            .as_ref()
            .ok_or("No compactor configured")?;
        let old_count = self.message_buffer.len();
        let prev_state = self.context.current_state();
        if !self.context.transition_to(AgentState::Compacting) {
            tracing::warn!("Failed to transition to Compacting from {:?}", prev_state);
        }
        self.emit_compaction_event(true).await;

        let result = compactor
            .full_compact(
                self.message_buffer.messages(),
                Arc::clone(&self.shared.provider),
                &self.shared.model_config,
                Some(self.cancel_token.runtime_token()),
            )
            .await
            .map(Some);

        if !self.context.transition_to(prev_state) {
            tracing::warn!(
                "Failed to transition back to {:?} from Compacting",
                prev_state
            );
        }
        self.emit_compaction_event(false).await;
        self.handle_compaction_result(result, old_count).await
    }

    /// Handle compaction result, update state, and return user message.
    /// Clears file state store only if messages were actually reduced (real compaction).
    #[tracing::instrument(skip(self))]
    async fn handle_compaction_result(
        &mut self,
        result: Result<Option<crate::compactor::CompactionResult>, CompactionError>,
        old_count: usize,
    ) -> Result<String, String> {
        let compact_result = match result {
            Ok(None) => Ok("No compaction needed".to_string()),
            Ok(Some(compaction_result)) => {
                // Record compactor token usage
                self.record_compactor_token_usage(compaction_result.token_usage)
                    .await;

                self.apply_compacted_messages(compaction_result.messages)
                    .await;
                let new_count = self.message_buffer.len();
                let compacted_count = old_count.saturating_sub(new_count);

                // Clear file state only if messages were actually reduced (real compaction)
                if compacted_count > 0 {
                    if let Some(ref file_state_store) = self.shared.file_state_store {
                        tracing::info!(
                            "clearing file state due to compaction ({} -> {} messages)",
                            old_count,
                            new_count
                        );
                        file_state_store.clear().await;
                    }
                }

                Ok(if compacted_count > 0 {
                    info!(
                        "compaction completed: {} -> {} messages (compacted {})",
                        old_count, new_count, compacted_count
                    );
                    format!("Compacted {compacted_count} messages")
                } else {
                    "Micro-compaction completed".to_string()
                })
            }
            Err(CompactionError::Cancelled) => {
                tracing::info!("compaction cancelled");
                self.emit_operation_cancelled("compaction").await;
                Err("Compaction was cancelled".to_string())
            }
            Err(CompactionError::Api(e)) => {
                tracing::warn!("compaction failed: {}", e);
                self.emit_error(crate::event::ErrorPhase::Compaction, &e.clone(), false)
                    .await;
                Err(format!("Compaction failed: {e}"))
            }
        };

        compact_result
    }

    /// Emit compaction start/end event.
    async fn emit_compaction_event(&self, active: bool) {
        if let Err(e) = self
            .event_bus
            .try_send(Event::Model(ModelEvent::Compacting {
                agent_id: self.id.clone(),
                active,
            }))
        {
            tracing::warn!("Failed to send compacting event (active={}): {}", active, e);
        }
    }

    /// Record compactor token usage
    async fn record_compactor_token_usage(&self, usage: crate::providers::TokenUsage) {
        if usage.prompt_tokens == 0 && usage.completion_tokens == 0 {
            return; // No usage to record
        }
        if let Some(store) = &self.shared.usage_store {
            let record = crate::storage::UsageRecord::new(
                self.session_id.clone(),
                self.id.clone(),
                usage,
                self.shared.model_config.model_id.clone(),
                self.shared.model_config.provider.to_string(),
                crate::storage::UsageType::Compactor,
            );
            if let Err(e) = store.record(&record).await {
                tracing::warn!("Failed to record compactor token usage: {}", e);
            }
        }
    }

    /// Apply compacted messages: update buffer and persist to storage.
    /// Note: Preserves the system message at the beginning of the buffer.
    async fn apply_compacted_messages(&mut self, messages: Vec<Arc<Message>>) {
        // Reconstruct buffer: keep system messages + compacted messages (filter out any system msgs from compactor)
        let new_messages: Vec<Arc<Message>> = self
            .message_buffer
            .messages()
            .iter()
            .filter(|m| m.role == Role::System)
            .take(1) // Only keep the first system message (the original prompt)
            .cloned()
            .chain(messages.iter().filter(|m| m.role != Role::System).cloned())
            .collect();
        *self.message_buffer.messages_mut() = new_messages;

        // Persist compacted messages (without system messages)
        if let Some(store) = &self.shared.message_store {
            let to_persist: Vec<Message> = messages
                .into_iter()
                .filter(|m| m.role != Role::System)
                .map(|m| (*m).clone())
                .collect();
            if let Err(e) = store.replace(&self.session_id, &to_persist).await {
                tracing::warn!("failed to persist compacted messages: {}", e);
            }
        }
    }

    /// Check and run compaction if needed
    /// Returns true if compaction occurred (including full compaction)
    async fn maybe_compact_messages(&mut self) -> bool {
        let Some(compactor) = self.shared.compactor.as_ref() else {
            return false; // No compactor configured, skip
        };
        let should_compact = compactor.should_compact(self.message_buffer.messages());
        if !should_compact {
            return false;
        }
        // force_compact handles its own start/end events
        match self.force_compact().await {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("auto-compaction failed: {}", e);
                false
            }
        }
    }

    /// Transition to appropriate state after streaming completes
    #[tracing::instrument(skip(self))]
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

        if has_tool_calls {
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

        // No tool calls and no finish reason: the model likely stopped mid-stream
        // (e.g., hit max_tokens). Auto-inject a "continue" user message to resume.
        if matches!(finish_reason, None | Some(FinishReason::MaxTokens)) {
            tracing::info!(?finish_reason, "auto-injecting 'continue' user message");
            let msg = Message::user("continue");
            self.emit_user_message_event(&msg.id, &msg.content);
            self.persist_message(&msg).await;
            self.message_buffer.push(msg);
            self.context.transition_to(AgentState::Streaming);
            return Ok(());
        }

        // No tool calls: check PreStop hooks for goal auto-continue
        let ctx = crate::hooks::HookContext::pre_stop(&self.session_id.0, &self.data_dir);
        let result = self.hook_registry.run_pre_stop(&ctx).await;

        if let crate::hooks::HookResult::PreStop(decision) = result {
            if decision.continue_session {
                let steer_blocks = decision.steer_blocks.unwrap_or(Vec::new());
                if !steer_blocks.is_empty() {
                    let msg = Message::with_blocks(Role::User, steer_blocks);
                    self.persist_message(&msg).await;
                    self.message_buffer.push(msg);
                }
                tracing::info!(
                    "PreStop hooks decided to continue session, transitioning to Streaming"
                );
                self.context.transition_to(AgentState::Streaming);
                return Ok(());
            }
        }

        // No hook or hook says stop
        // If a goal exists and is not Active, emit GoalUpdated so frontends know
        if let Some(ref store) = self.shared.goal_store {
            if let Ok(Some(goal)) = store.load(&self.session_id).await {
                if !matches!(goal.status, crate::goal::GoalStatus::Active) {
                    let _ = self.event_bus.try_send(Event::System(
                        crate::event::SystemEvent::GoalUpdated {
                            session_id: self.session_id.clone(),
                            description: goal.description.clone(),
                            status: goal.status.as_str().to_string(),
                        },
                    ));
                }
            }
        }
        self.emit_stopped_completed(finish_reason);
        self.context.transition_to(AgentState::Idle);
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn handle_execute_tool(&mut self) -> Result<(), AgentError> {
        // Early-out if cancelled before doing any work
        if self.cancel_token.is_cancelled() {
            return Err(AgentError::Cancelled("tool execution".into()));
        }

        let tool_calls: Vec<_> = self
            .message_buffer
            .messages()
            .last()
            .and_then(|m| m.tool_calls.clone())
            .unwrap_or_default();

        // Pre-generate MessageId for each tool call so Start/End events and
        // the resulting Message all share the same identifier.
        let mut tool_message_ids: BTreeMap<String, MessageId> = BTreeMap::new();
        for call in &tool_calls {
            tool_message_ids.insert(call.id.clone(), MessageId::new());
        }

        // First: Send Started event for ALL tool calls (before permission check)
        for call in &tool_calls {
            let args_str = serde_json::to_string(&call.arguments).ok();
            let message_id = tool_message_ids[&call.id].clone();
            if let Err(e) = self.event_bus.try_send(Event::Tool(ToolEvent::Start {
                agent_id: self.id.clone(),
                message_id,
                tool_id: call.id.clone(),
                tool_name: call.name.clone(),
                arguments: args_str,
            })) {
                tracing::warn!("Failed to send tool start event: {}", e);
            }
        }

        // Check permissions for each tool call
        let permission_result = crate::permissions::check_tool_permissions(
            &tool_calls,
            self.permission_checker.as_deref(),
            &self.id,
        )
        .await;

        let mut approved_calls = permission_result.approved;
        let mut denied_results: Vec<_> = permission_result
            .denied
            .into_iter()
            .map(|(tool_call_id, error_msg)| {
                let message_id = tool_message_ids[&tool_call_id].clone();
                let message = Message::tool_result(
                    message_id.clone(),
                    tool_call_id.clone(),
                    error_msg.clone(),
                );
                ToolExecutionResult {
                    tool_call_id: tool_call_id.clone(),
                    message_id: message_id.clone(),
                    event: ToolEvent::End {
                        agent_id: self.id.clone(),
                        message_id,
                        tool_id: tool_call_id.clone(),
                        tool_name: String::new(),
                        content_blocks: vec![crate::types::ToolOutputBlock::Text {
                            text: error_msg.clone(),
                        }],
                        elapsed_ms: 0,
                        is_error: true,
                    },
                    message,
                }
            })
            .collect();

        // === PreToolUse hooks ===
        approved_calls = super::hooks::run_pre_tool_hooks(
            &self.id,
            &self.session_id,
            &self.working_dir,
            &self.hook_registry,
            approved_calls,
            &mut denied_results,
        )
        .await;

        // Create runtime token for this tool execution batch
        let cancel_token = self.create_runtime_token();

        // Execute only approved calls
        // Share current_turn with tools for file tracking
        let turn_for_tools = self.current_turn.clone();
        let results = if approved_calls.is_empty() {
            Vec::new()
        } else {
            crate::tools::execute_tools_parallel(&ToolExecParams {
                agent_id: &self.id,
                tool_calls: &approved_calls,
                tool_registry: &self.tool_registry,
                cancel_token: Some(&cancel_token),
                parent_messages: Some(self.message_buffer.messages()),
                working_dir: &self.working_dir,
                session_id: &self.session_id,
                message_ids: &tool_message_ids,
                turn: turn_for_tools,
                skills: &self.skills,
                max_tool_output_length: self.max_tool_output_length,
            })
            .await
        };

        // Track files for checkpointing (via current_turn if exists)
        // Note: In the new design, tools should call track_file via the turn directly
        // For now, we track files here after tool execution completes

        // === PostToolUse hooks ===
        let (post_results, continue_session, post_contexts) = super::hooks::run_post_tool_hooks(
            &self.id,
            &self.session_id,
            &self.working_dir,
            &self.hook_registry,
            results,
            &tool_calls,
        )
        .await;

        // Combine denied and executed results
        let all_results: Vec<_> = denied_results.into_iter().chain(post_results).collect();

        for result in all_results {
            if self.cancel_token.is_cancelled() {
                return Err(AgentError::Cancelled("tool execution".into()));
            }
            if let Err(e) = self.event_bus.try_send(Event::Tool(result.event)) {
                tracing::warn!("Failed to send tool end event: {}", e);
            }
            self.persist_message(&result.message).await;
            self.message_buffer.push(result.message);
        }

        // Inject PostToolUse hook contexts as independent messages after all
        // tool results. This keeps the tool call chain contiguous so
        // `sanitize()` won't strip the chain.
        for ctx_text in post_contexts {
            let msg = Message::user(ctx_text);
            self.persist_message(&msg).await;
            self.message_buffer.push(msg);
        }

        if !continue_session {
            tracing::info!("stopping after tool execution (hook requested)");
            self.context.transition_to(AgentState::Idle);
            return Ok(());
        }

        // PostToolUse says continue → always transition back to Streaming.
        // The PreStop hook (goal check) runs at the end of streaming only.
        self.context.transition_to(AgentState::Streaming);
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn handle_streaming_with_retry(&mut self) -> Result<(), AgentError> {
        let max_retries = 10;
        let mut attempt = 0;

        loop {
            match self.handle_streaming().await {
                Ok(()) => return Ok(()),
                Err(e) if e.is_cancelled() => return Err(e),
                Err(e) if attempt >= max_retries => {
                    return self
                        .fail_agent("Streaming failed after max retries", e)
                        .await;
                }
                Err(e) if !e.is_retryable() => {
                    return self
                        .fail_agent("Streaming failed with non-retryable error", e)
                        .await;
                }
                Err(e) => {
                    attempt += 1;
                    tracing::warn!("Streaming failed (attempt {}), retrying: {}", attempt, e);
                    self.emit_retrying(attempt, max_retries, &e.to_string())
                        .await;
                    self.emit_error(crate::event::ErrorPhase::Streaming, &e.to_string(), true)
                        .await;
                    tokio::time::sleep(tokio::time::Duration::from_secs(u64::from(attempt))).await;
                }
            }
        }
    }
}
