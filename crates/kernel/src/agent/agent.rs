use super::message_buffer::MessageBuffer;
use super::{
    AgentError, AgentExecutionContext, AgentHandle, AgentShared, AgentSpawnArgs, AgentState,
    CancelToken,
};
use crate::compactor::{CompactionError, DEFAULT_CONTEXT_WINDOW};
use crate::event::{AgentEvent, AgentResult, Event, ModelEvent, ToolEvent};
use crate::permissions::Checker;
use crate::prompt::SystemPromptBuilder;
use crate::tools::parallel::ToolExecutionResult;
use crate::types::{AgentId, ContentBlock, Message, MessageTokenUsage, Role, ToolCall};
use futures::TryStreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Input messages that can be sent to an Agent
#[derive(Debug, Clone)]
pub enum AgentInput {
    /// User message with multi-modal content blocks
    User(Vec<ContentBlock>),
    /// Background task completion
    TaskResult {
        task_id: String,
        content: Vec<ContentBlock>,
    },
    /// Permission response from user/TUI
    PermissionResponse { req_id: String, approved: bool },
    /// Close the agent gracefully (for subagent/resource management)
    Close,
    /// Force compaction of message buffer
    Compact,
}

pub struct Agent {
    id: AgentId,
    shared: Arc<AgentShared>,
    message_buffer: MessageBuffer,
    event_tx: mpsc::Sender<Event>,
    input_rx: mpsc::Receiver<AgentInput>,
    context: AgentExecutionContext,
    cancel_token: CancelToken,
    session_id: String,
    max_iterations: usize,
    // Tool registry - each agent has its own set of tools
    tool_registry: crate::tools::ToolRegistry,
    // Permission checker for tool execution
    permission_checker: Option<Arc<Checker>>,
    // Pending token usage for the current message
    pending_token_usage: Option<MessageTokenUsage>,
}

impl Agent {
    pub fn spawn(
        id: AgentId,
        shared: &Arc<AgentShared>,
        args: AgentSpawnArgs,
    ) -> (AgentHandle, mpsc::Receiver<Event>) {
        let (input_tx, input_rx) = mpsc::channel::<AgentInput>(20);
        let (event_tx, event_rx) = mpsc::channel(100);
        let cancel_token = args.cancel_token.clone().unwrap_or_default();
        let (context, state_rx) = AgentExecutionContext::new(AgentState::Idle);

        // Build system prompt with project memory and skills
        let system_prompt = if shared.project_memory.is_empty() {
            // No project memory, use original builder
            SystemPromptBuilder::new()
                .base_prompt(&args.base_prompt)
                .with_skills(&args.skills)
                .build()
        } else {
            // Merge project memory with base prompt
            let memory_prompt = shared.project_memory.build_system_prompt(&args.base_prompt);
            // Add skills after project memory
            SystemPromptBuilder::new()
                .base_prompt(&memory_prompt)
                .with_skills(&args.skills)
                .build()
        };

        tracing::debug!(
            "Agent {} spawning with system prompt: {}",
            id,
            system_prompt
        );
        let mut messages: Vec<Arc<Message>> = vec![Arc::new(Message::system(system_prompt))];
        messages.extend(args.history.into_iter().filter(|m| m.role != Role::System));
        let message_buffer = MessageBuffer::from_arc_messages(&messages);

        let shared = shared.clone();

        // Create agent-specific tool registry with standard tools
        let tool_registry = crate::tools::ToolRegistryFactory::create(
            &id,
            &shared,
            &args.working_dir,
            Some(&input_tx),
            &event_tx,
            args.skills.clone(),
            &args.session_id,
            args.parent_session_id.as_deref(),
            args.enable_sub_agents,
            shared.skill_folders.clone(),
            args.file_state_store.clone(),
        );

        // Create permission checker and responder from shared state
        // If no permission_state in shared (YOLO mode), all tools auto-approve
        let (permission_checker, permission_responder) = match shared.permission_state.as_ref() {
            Some(state) => {
                let checker = Checker::new(state.clone(), id.clone(), event_tx.clone());
                let responder = state.create_responder();
                (Some(Arc::new(checker)), Some(responder))
            }
            None => (None, None),
        };

        let agent = Self {
            id: id.clone(),
            shared,
            message_buffer,
            event_tx,
            input_rx,
            context,
            cancel_token: cancel_token.clone(),
            session_id: args.session_id,
            max_iterations: args.max_iterations,
            tool_registry,
            permission_checker,
            pending_token_usage: None,
        };

        let handle_id = id.clone();
        tokio::spawn(async move {
            if let Err(e) = agent.run().await {
                tracing::error!("Agent {} failed: {}", handle_id, e);
            }
            info!("Agent {} closed", handle_id);
        });

        let handle = AgentHandle::new(id, input_tx, state_rx, cancel_token, permission_responder);
        (handle, event_rx)
    }

    /// Create a runtime `CancellationToken` linked to the Agent's custom `CancelToken`.
    ///
    /// This bridges the Agent layer (with reset support) to the Runtime layer
    /// (using tokio native `CancellationToken`).
    fn create_runtime_token(&self) -> tokio_util::sync::CancellationToken {
        // 直接获取父 Agent 的 tokio CancellationToken
        // 不需要 spawn 桥接任务，因为 CancelToken 内部就是 CancellationToken
        self.cancel_token.runtime_token()
    }

    /// Persist a single message to storage
    async fn persist_message(&self, message: &Message) {
        if let Some(storage) = &self.shared.storage {
            let session_id = crate::types::SessionId(self.session_id.clone());
            let _ = storage
                .append_messages(&session_id, std::slice::from_ref(message))
                .await;
        }
    }

    async fn run(mut self) -> Result<(), AgentError> {
        tracing::info!(
            "Agent {} started with {} messages",
            self.id,
            self.message_buffer.len()
        );

        self.context.transition_to(AgentState::WaitingForInput);
        loop {
            let state = self.context.current_state();

            if state.is_terminal() {
                break;
            }

            if self.context.iteration_count() >= self.max_iterations
                && self.context.current_state() == AgentState::Streaming
            {
                tracing::warn!(
                    "Agent {} reached max iterations during streaming, cancelling and returning to waiting for input",
                    self.id
                );
                // Notify TUI that max iterations reached
                let _ = self
                    .event_tx
                    .send(Event::Agent(AgentEvent::MaxIterationsReached {
                        agent_id: self.id.clone(),
                        count: self.max_iterations,
                    }))
                    .await;
                self.context.transition_to(AgentState::WaitingForInput);
            }

            // Note: cancel is handled during streaming via select!, not here
            // This prevents the token from getting stuck in cancelled state

            match state {
                AgentState::WaitingForInput => {
                    self.context.reset_iteration();
                    tracing::debug!("Agent {} waiting for input", self.id);
                    if let Err(e) = self.handle_wait_for_input().await {
                        self.emit_error(
                            crate::event::ErrorPhase::WaitForInput,
                            &e.to_string(),
                            false,
                        )
                        .await;
                    }
                }
                AgentState::Streaming => {
                    tracing::debug!("Agent {} starting streaming", self.id);
                    if let Err(e) = self.handle_streaming_with_retry().await {
                        self.emit_error(crate::event::ErrorPhase::Streaming, &e.to_string(), false)
                            .await;
                        self.context.transition_to(AgentState::WaitingForInput);
                    }
                }
                AgentState::ExecutingTool => {
                    tracing::info!("Agent {} executing tools", self.id);
                    if let Err(e) = self.handle_execute_tool().await {
                        self.emit_error(
                            crate::event::ErrorPhase::ToolExecution,
                            &e.to_string(),
                            false,
                        )
                        .await;
                        self.context.transition_to(AgentState::WaitingForInput);
                    }
                }
                _ => tokio::task::yield_now().await,
            }

            self.context.increment_iteration();
        }

        let tool_calls = self.count_tool_calls();
        let final_state = self.context.current_state();
        tracing::info!(
            "Agent {} finished: state={:?}, messages={}, tool_calls={}",
            self.id,
            final_state,
            self.message_buffer.len(),
            tool_calls
        );

        let result = AgentResult {
            messages: self.message_buffer.messages().to_vec(),
            tool_calls,
        };
        let _ = self
            .event_tx
            .send(Event::Agent(AgentEvent::Completed {
                agent_id: self.id.clone(),
                result,
            }))
            .await;

        Ok(())
    }

    /// Handle cancellation - sends Cancelled event, transitions state, returns Ok(())
    async fn handle_cancel(&self, context: &str) -> Result<(), AgentError> {
        tracing::info!("Agent {} {} cancelled", self.id, context);
        // 发送带 operation 的取消事件
        self.emit_operation_cancelled(context).await;
        self.context.transition_to(AgentState::WaitingForInput);
        Ok(())
    }

    /// Helper to emit `AgentEvent::Failed` and return error
    async fn fail_agent(&self, context: &str, error: AgentError) -> Result<(), AgentError> {
        let error_msg = format!("{context}: {error}");
        tracing::error!("Agent {} failed: {}", self.id, error_msg);
        let _n = self
            .event_tx
            .send(Event::Agent(AgentEvent::Failed {
                agent_id: self.id.clone(),
                error: error_msg,
            }))
            .await;
        Err(error)
    }

    /// Emit error event (recoverable or not) and log it
    async fn emit_error(&self, phase: crate::event::ErrorPhase, error: &str, is_recoverable: bool) {
        if is_recoverable {
            tracing::warn!(
                "Agent {} {:?} error (recoverable): {}",
                self.id,
                phase,
                error
            );
        } else {
            tracing::error!("Agent {} {:?} error: {}", self.id, phase, error);
        }

        if let Err(e) = self.event_tx.try_send(Event::Agent(AgentEvent::Error {
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
        if let Err(e) = self.event_tx.try_send(Event::Agent(AgentEvent::Retrying {
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
        if let Err(e) = self.event_tx.try_send(Event::Agent(AgentEvent::Cancelled {
            agent_id: self.id.clone(),
            operation: Some(operation.to_string()),
        })) {
            tracing::warn!("Failed to emit operation cancelled event: {}", e);
        }
    }

    async fn handle_wait_for_input(&mut self) -> Result<(), AgentError> {
        match self.input_rx.recv().await {
            Some(AgentInput::User(content)) => {
                self.cancel_token.reset_if_cancelled();
                let text_content = content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let _ = self
                    .event_tx
                    .send(Event::User(crate::event::UserEvent::Message {
                        content: text_content,
                    }))
                    .await;
                let msg = Message::with_blocks(Role::User, content);
                self.persist_message(&msg).await;
                self.message_buffer.push(msg);
                self.context.transition_to(AgentState::Streaming);
                Ok(())
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
                // PermissionResponse 现在通过 PermissionResponder 处理
                // 保留此方法以防需要特殊处理
                tracing::warn!("Agent {} received PermissionResponse via input channel (should use PermissionResponder instead): req_id={}", self.id, req_id);
                Ok(())
            }
            Some(AgentInput::Close) => {
                tracing::info!("Agent {} received close signal", self.id);
                self.context.transition_to(AgentState::Closed);
                Ok(())
            }
            Some(AgentInput::Compact) => {
                tracing::info!("Agent {} received compact request", self.id);
                if let Err(e) = self.force_full_compact().await {
                    tracing::warn!("Agent {} force_full_compact failed: {}", self.id, e);
                }
                // User-initiated compact doesn't auto-continue, stay in WaitingForInput
                Ok(())
            }
            None => {
                self.context.transition_to(AgentState::Closed);
                Ok(())
            }
        }
    }

    async fn handle_streaming(&mut self) -> Result<(), AgentError> {
        // 1. Check and run compaction if needed (at the very beginning)
        if self.maybe_compact_messages().await {
            tracing::info!(
                "Agent {} performed auto-compaction before streaming",
                self.id
            );
        }

        // 2. Prepare streaming
        let tools = self.tool_registry.definitions();
        tracing::info!(
            "Agent {} preparing to stream with {} tool(s): {:?}",
            self.id,
            tools.len(),
            tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );

        let _ = self
            .event_tx
            .send(Event::Model(ModelEvent::Request {
                agent_id: self.id.clone(),
                message_count: self.message_buffer.len(),
            }))
            .await;

        // Validate and clean message buffer before sending to provider
        self.message_buffer.validate_and_clean();

        // Clone messages and tools for the spawned task (needs 'static)
        let messages: Vec<Arc<Message>> = self.message_buffer.messages().to_vec();

        // Spawn provider request in a separate task to allow cancellation
        let provider = self.shared.provider.clone();
        let model_config = self.shared.model_config.clone();
        let stream_task =
            tokio::spawn(async move { provider.stream(&messages, &tools, &model_config).await });
        let abort_handle = stream_task.abort_handle();

        info!("Agent {} waiting for model stream to start", self.id);

        let mut stream = tokio::select! {
            biased;
            () = self.cancel_token.cancelled() => {
                abort_handle.abort();
                return self.handle_cancel("stream creation").await;
            }
            result = stream_task => match result {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => return Err(AgentError::Provider(e)),
                Err(e) if e.is_cancelled() => {
                    return Err(AgentError::Cancelled);
                }
                Err(e) => return Err(AgentError::StreamTaskPanicked(e.to_string())),
            }
        };

        let (content_blocks, pending_tool_calls) = self.collect_stream_output(&mut stream).await?;

        if !content_blocks.is_empty() || !pending_tool_calls.is_empty() {
            let mut msg = Message::with_blocks(Role::Assistant, content_blocks);
            if !pending_tool_calls.is_empty() {
                msg.tool_calls = Some(pending_tool_calls);
            }
            if let Some(usage) = self.pending_token_usage.take() {
                msg.token_usage = Some(usage);
            }

            self.persist_message(&msg).await;
            self.message_buffer.push(msg);
        }

        self.transition_after_streaming().await
    }

    /// Collect all output from the stream until completion
    async fn collect_stream_output(
        &mut self,
        stream: &mut crate::providers::ModelStream,
    ) -> Result<(Vec<ContentBlock>, Vec<ToolCall>), AgentError> {
        use super::stream_collector::StreamCollectorState;
        use crate::providers::ModelStreamItem;

        let mut state = StreamCollectorState::default();

        loop {
            tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    return self.handle_cancel("streaming").await.map(|()| (vec![], vec![]));
                }
                item = stream.try_next() => match item {
                    Ok(Some(item)) => match item {
                        ModelStreamItem::Chunk(chunk) => {
                            state.handle_chunk(&chunk);
                            if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::Chunk {
                                agent_id: self.id.clone(),
                                content: chunk,
                            })) {
                                tracing::warn!("Failed to send chunk event: {}", e);
                            }
                        }
                        ModelStreamItem::ToolCall(request) => {
                            state.handle_tool_call(request);
                        }
                        ModelStreamItem::Complete => break,
                        ModelStreamItem::Fallback { from, to } => {
                            if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::Fallback {
                                agent_id: self.id.clone(),
                                from,
                                to,
                            })) {
                                tracing::warn!("Failed to send fallback event: {}", e);
                            }
                        }
                        ModelStreamItem::TokenUsage { prompt_tokens, completion_tokens } => {
                            // NOTE: this is right because each response's prompt_tokens will contain whole history
                            let total = prompt_tokens + completion_tokens;
                            self.pending_token_usage = Some(MessageTokenUsage {
                                prompt_tokens,
                                completion_tokens,
                                total_tokens: total,
                            });
                            state.handle_token_usage(prompt_tokens, completion_tokens);
                            // Get context window from compactor or use default
                            let context_window = self.shared.compactor.as_ref()
                                .map_or(DEFAULT_CONTEXT_WINDOW, |c| c.context_window);
                            if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::TokenUsage {
                                agent_id: self.id.clone(),
                                prompt_tokens,
                                completion_tokens,
                                total_tokens: total,
                                context_window,
                            })) {
                                tracing::warn!("Failed to send token usage event: {}", e);
                            }
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
        Ok((result.content_blocks, result.tool_calls))
    }

    /// Force compaction regardless of threshold.
    pub async fn force_compact(&mut self) -> Result<String, String> {
        let compactor = self
            .shared
            .compactor
            .as_ref()
            .ok_or("No compactor configured")?;
        let old_count = self.message_buffer.len();

        self.emit_compaction_event(true).await;

        let result = compactor
            .auto_compact(
                self.message_buffer.messages(),
                &*self.shared.provider,
                &self.shared.model_config,
                Some(self.cancel_token.runtime_token()),
            )
            .await;

        self.handle_compaction_result(result, old_count).await
    }

    /// Force full compaction (skip micro-compaction).
    pub async fn force_full_compact(&mut self) -> Result<String, String> {
        let compactor = self
            .shared
            .compactor
            .as_ref()
            .ok_or("No compactor configured")?;
        let old_count = self.message_buffer.len();

        self.emit_compaction_event(true).await;

        let result = compactor
            .full_compact(
                self.message_buffer.messages(),
                &*self.shared.provider,
                &self.shared.model_config,
                Some(self.cancel_token.runtime_token()),
            )
            .await
            .map(Some);

        self.handle_compaction_result(result, old_count).await
    }

    /// Handle compaction result, update state, and return user message.
    async fn handle_compaction_result(
        &mut self,
        result: Result<Option<Vec<Message>>, CompactionError>,
        old_count: usize,
    ) -> Result<String, String> {
        let compact_result = match result {
            Ok(None) => Ok("No compaction needed".to_string()),
            Ok(Some(messages)) => {
                let compacted_count = old_count.saturating_sub(messages.len());
                self.apply_compacted_messages(messages).await;

                Ok(if compacted_count > 0 {
                    format!("Compacted {compacted_count} messages")
                } else {
                    "Micro-compaction completed".to_string()
                })
            }
            Err(CompactionError::Cancelled) => {
                tracing::info!("Agent {} compaction cancelled", self.id);
                self.emit_operation_cancelled("compaction").await;
                Err("Compaction was cancelled".to_string())
            }
            Err(CompactionError::Api(e)) => {
                tracing::warn!("Agent {} compaction failed: {}", self.id, e);
                self.emit_error(crate::event::ErrorPhase::Compaction, &e.to_string(), false)
                    .await;
                Err(format!("Compaction failed: {e}"))
            }
        };

        self.emit_compaction_event(false).await;
        compact_result
    }

    /// Emit compaction start/end event.
    async fn emit_compaction_event(&self, active: bool) {
        if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::Compacting {
            agent_id: self.id.clone(),
            active,
        })) {
            tracing::warn!("Failed to send compacting event (active={}): {}", active, e);
        }
    }

    /// Apply compacted messages: update buffer and persist to storage.
    async fn apply_compacted_messages(&mut self, messages: Vec<Message>) {
        let compacted_count = self.message_buffer.len().saturating_sub(messages.len());
        if compacted_count > 0 {
            tracing::info!(
                "Agent {} compacted {} messages -> {} messages",
                self.id,
                self.message_buffer.len(),
                messages.len()
            );
        }

        // Update message buffer
        *self.message_buffer.messages_mut() = messages.iter().cloned().map(Arc::new).collect();

        // Persist compacted state
        if let Some(storage) = &self.shared.storage {
            let sid = crate::types::SessionId(self.session_id.clone());
            if let Err(e) = storage.set_messages(&sid, &messages).await {
                tracing::warn!(
                    "Agent {} failed to persist compacted messages: {}",
                    self.id,
                    e
                );
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
                tracing::warn!("Agent {} auto-compaction failed: {}", self.id, e);
                false
            }
        }
    }

    /// Transition to appropriate state after streaming completes
    async fn transition_after_streaming(&self) -> Result<(), AgentError> {
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
                .unwrap()
                .tool_calls
                .as_ref()
                .map_or(0, std::vec::Vec::len);
            tracing::info!(
                "Agent {} detected {} tool call(s), transitioning to ExecutingTool",
                self.id,
                tool_count
            );
            self.context.transition_to(AgentState::ExecutingTool);
        } else {
            tracing::info!(
                "Agent {} streaming complete, waiting for next input",
                self.id
            );
            if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::Completed {
                agent_id: self.id.clone(),
            })) {
                tracing::warn!("Failed to send completed event: {}", e);
            }
            self.context.transition_to(AgentState::WaitingForInput);
        }
        Ok(())
    }

    async fn handle_execute_tool(&mut self) -> Result<(), AgentError> {
        let tool_calls = self
            .message_buffer
            .messages()
            .last()
            .and_then(|m| m.tool_calls.as_deref())
            .unwrap_or(&[]);

        // First: Send Started event for ALL tool calls (before permission check)
        // This ensures the UI shows all tools that are being attempted
        for call in tool_calls {
            let args_str = serde_json::to_string(&call.arguments).ok();
            let _ = self
                .event_tx
                .send(Event::Tool(ToolEvent::Started {
                    agent_id: self.id.clone(),
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    arguments: args_str,
                }))
                .await;
        }

        // Check permissions for each tool call
        let permission_result = crate::permissions::check_tool_permissions(
            tool_calls,
            self.permission_checker.as_deref(),
            &self.id,
        )
        .await;

        let approved_calls = permission_result.approved;
        let denied_results: Vec<_> = permission_result
            .denied
            .into_iter()
            .map(|(tool_call_id, error_msg)| ToolExecutionResult {
                tool_call_id: tool_call_id.clone(),
                event: ToolEvent::Error {
                    agent_id: self.id.clone(),
                    tool_id: tool_call_id.clone(),
                    error: error_msg.clone(),
                    content_blocks: Vec::new(),
                    elapsed_ms: 0,
                },
                message: Message::tool_result(tool_call_id, error_msg),
            })
            .collect();

        // Create runtime token for this tool execution batch
        let cancel_token = self.create_runtime_token();

        // Execute only approved calls
        let results = if approved_calls.is_empty() {
            Vec::new()
        } else {
            crate::tools::execute_tools_parallel(
                &self.id,
                &approved_calls,
                &self.tool_registry,
                Some(&cancel_token),
                Some(self.message_buffer.messages()),
            )
            .await
        };

        // Combine denied and executed results
        let all_results: Vec<_> = denied_results.into_iter().chain(results).collect();

        for result in all_results {
            if self.cancel_token.is_cancelled() {
                return self.handle_cancel("tool execution").await;
            }
            let _ = self.event_tx.send(Event::Tool(result.event)).await;
            self.persist_message(&result.message).await;
            self.message_buffer.push(result.message);
        }

        self.context.transition_to(AgentState::Streaming);
        Ok(())
    }

    fn count_tool_calls(&self) -> usize {
        self.message_buffer
            .messages()
            .iter()
            .filter_map(|m| m.tool_calls.as_ref().map(std::vec::Vec::len))
            .sum()
    }

    #[allow(dead_code)]
    fn messages(&self) -> &[Arc<Message>] {
        self.message_buffer.messages()
    }

    async fn handle_streaming_with_retry(&mut self) -> Result<(), AgentError> {
        let max_retries = 10;
        let mut attempt = 0;

        loop {
            match self.handle_streaming().await {
                Ok(()) => return Ok(()),
                Err(e) if attempt >= max_retries => {
                    // 发送不可恢复错误事件
                    self.emit_error(crate::event::ErrorPhase::Streaming, &e.to_string(), false)
                        .await;
                    return self
                        .fail_agent("Streaming failed after max retries", e)
                        .await;
                }
                Err(e) if !Self::is_retryable_error(&e) => {
                    // 发送不可恢复错误事件
                    self.emit_error(crate::event::ErrorPhase::Streaming, &e.to_string(), false)
                        .await;
                    return self
                        .fail_agent("Streaming failed with non-retryable error", e)
                        .await;
                }
                Err(e) => {
                    attempt += 1;
                    // 发送重试事件
                    self.emit_retrying(attempt, max_retries, &e.to_string())
                        .await;
                    // 发送可恢复错误事件
                    self.emit_error(crate::event::ErrorPhase::Streaming, &e.to_string(), true)
                        .await;
                    tracing::warn!("Streaming failed (attempt {}), retrying: {}", attempt, e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(u64::from(attempt))).await;
                }
            }
        }
    }

    /// Check if an error is retryable.
    fn is_retryable_error(error: &AgentError) -> bool {
        warn!("Streaming error: {}", error);
        error.is_retryable()
    }
}
