use super::message_buffer::MessageBuffer;
use super::{
    AgentError, AgentExecutionContext, AgentHandle, AgentShared, AgentSpawnArgs, AgentState,
    CancelToken,
};
use crate::compactor;
use crate::event::{AgentEvent, AgentResult, ContentChunk, Event, ModelEvent, ToolEvent};
use crate::permissions::{Checker, ToolLevelResolver};
use crate::prompt::SystemPromptBuilder;
use crate::providers::ModelStreamItem;
use crate::skill::Skill;
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
    // Store the last error message for display
    last_error: Option<String>,
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
        let message_buffer = MessageBuffer::from_arc_messages(messages);

        let shared = shared.clone();

        // Create agent-specific tool registry with standard tools
        let tool_registry = Self::create_tool_registry(
            &id,
            &shared,
            &args.working_dir,
            &input_tx,
            &event_tx,
            args.skills.clone(),
            &args.session_id,
            args.parent_session_id.as_deref(),
            args.enable_sub_agents,
            Some(&cancel_token),
        );

        // Create permission checker and responder from shared state
        // If no permission_state in shared (YOLO mode), all tools auto-approve
        // For subagents: use parent's event_tx so permission requests go to parent TUI
        let permission_event_tx = args
            .parent_event_tx
            .clone()
            .unwrap_or_else(|| event_tx.clone());
        let (permission_checker, permission_responder) = match shared.permission_state.as_ref() {
            Some(state) => {
                let checker = Checker::new(state.clone(), id.clone(), permission_event_tx);
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
            last_error: None,
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

    /// Create tool registry for an agent with standard tools
    #[allow(clippy::too_many_arguments)]
    fn create_tool_registry(
        agent_id: &AgentId,
        shared: &Arc<AgentShared>,
        working_dir: &std::path::PathBuf,
        input_tx: &mpsc::Sender<AgentInput>,
        event_tx: &mpsc::Sender<Event>,
        skills: Vec<Arc<Skill>>,
        session_id: &str,
        parent_session_id: Option<&str>,
        enable_sub_agents: bool,
        cancel_token: Option<&CancelToken>,
    ) -> crate::tools::ToolRegistry {
        use crate::tools::{
            BashTool, BashToolCtx, EditTool, GlobTool, GrepTool, ReadTool, SubagentTool, WriteTool,
        };

        let mut registry = crate::tools::ToolRegistry::new();
        let file_state_store = Arc::new(crate::tools::file_state::FileStateStore::new());

        // Register Bash tool
        let bash_ctx = BashToolCtx::new(agent_id.clone(), input_tx.clone(), working_dir.clone());
        let bash_tool = BashTool::new(working_dir).with_ctx(bash_ctx);
        registry.register(bash_tool);

        // Register Read tool with file state store
        let read_tool =
            ReadTool::new(working_dir).with_file_state_store(Arc::clone(&file_state_store));
        registry.register(read_tool);

        // Register Edit tool with file state store
        let edit_tool =
            EditTool::new(working_dir).with_file_state_store(Arc::clone(&file_state_store));
        registry.register(edit_tool);

        // Register Write tool with file state store
        let write_tool =
            WriteTool::new(working_dir).with_file_state_store(Arc::clone(&file_state_store));
        registry.register(write_tool);

        // Register Glob tool
        let glob_tool = GlobTool::new(working_dir);
        registry.register(glob_tool);

        // Register Grep tool
        let grep_tool = GrepTool::new(working_dir);
        registry.register(grep_tool);

        // Register SubAgent tool if enabled
        if enable_sub_agents {
            let subagent_tool = SubagentTool::new(
                agent_id.clone(),
                Arc::clone(shared),
                input_tx.clone(),
                skills,
                shared.storage.clone(),
                working_dir.clone(),
                session_id.to_owned(),
                event_tx.clone(),
                cancel_token.cloned(),
            );
            registry.register(subagent_tool);
        }

        // Register task tools if task_store is provided
        if let Some(task_store) = &shared.task_store {
            // Use parent_session_id for task store if available (subagents share parent's task list)
            let task_list_id = parent_session_id.unwrap_or(session_id).to_owned();
            registry.register_task_tools(task_store.clone(), task_list_id);
        }

        registry
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

            if self.context.iteration_count() >= self.max_iterations {
                tracing::warn!("Max iterations reached, forcing completion");
                self.context.transition_to(AgentState::Closed);
                break;
            }

            // Note: cancel is handled during streaming via select!, not here
            // This prevents the token from getting stuck in cancelled state

            match state {
                AgentState::WaitingForInput => {
                    tracing::debug!("Agent {} waiting for input", self.id);
                    if let Err(e) = self.handle_wait_for_input().await {
                        self.record_error("Wait for input failed", &e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                AgentState::Streaming => {
                    tracing::debug!("Agent {} starting streaming", self.id);
                    if let Err(e) = self.handle_streaming_with_retry().await {
                        self.record_error("Streaming failed", &e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                AgentState::ExecutingTool => {
                    tracing::info!("Agent {} executing tools", self.id);
                    if let Err(e) = self.handle_execute_tool().await {
                        self.record_error("Tool execution failed", &e);
                        self.context.transition_to(AgentState::Failed);
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

        // Send appropriate event based on final state
        match final_state {
            AgentState::Cancelled => {
                let _ = self
                    .event_tx
                    .send(Event::Agent(AgentEvent::Cancelled {
                        agent_id: self.id.clone(),
                    }))
                    .await;
            }
            AgentState::Failed => {
                let error_msg = self
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "Agent failed".to_string());
                let _ = self
                    .event_tx
                    .send(Event::Agent(AgentEvent::Failed {
                        agent_id: self.id.clone(),
                        error: error_msg,
                    }))
                    .await;
            }
            _ => {
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
            }
        }

        Ok(())
    }

    /// Handle cancellation - sends Cancelled event, transitions state, returns Ok(())
    async fn handle_cancel(&self, context: &str) -> Result<(), AgentError> {
        tracing::info!("Agent {} {} cancelled", self.id, context);
        let _ = self
            .event_tx
            .send(Event::Agent(AgentEvent::Cancelled {
                agent_id: self.id.clone(),
            }))
            .await;
        self.context.transition_to(AgentState::WaitingForInput);
        Ok(())
    }

    /// Record an error and store it for later display
    fn record_error(&mut self, context: &str, error: &AgentError) {
        let msg = format!("{context}: {error}");
        tracing::error!("Agent {} failed: {}", self.id, msg);
        self.last_error = Some(msg);
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

    async fn handle_wait_for_input(&mut self) -> Result<(), AgentError> {
        match self.input_rx.recv().await {
            Some(AgentInput::User(content)) => {
                self.cancel_token.reset();
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
                self.cancel_token.reset();
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
            None => {
                self.context.transition_to(AgentState::Cancelled);
                Ok(())
            }
        }
    }

    async fn handle_streaming(&mut self) -> Result<(), AgentError> {
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

            // Handle compaction if needed
            self.maybe_compact_messages().await;
        }

        self.transition_after_streaming().await
    }

    /// Collect all output from the stream until completion
    async fn collect_stream_output(
        &mut self,
        stream: &mut crate::providers::ModelStream,
    ) -> Result<(Vec<ContentBlock>, Vec<ToolCall>), AgentError> {
        let mut current_text = String::new();
        let mut current_thinking = String::new();
        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut pending_tool_calls: Vec<ToolCall> = Vec::new();

        loop {
            tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    return self.handle_cancel("streaming").await.map(|()| (vec![], vec![]));
                }
                item = stream.try_next() => match item {
                    Ok(Some(item)) => match item {
                        ModelStreamItem::Chunk(ContentChunk::Text(text)) => {
                            current_text.push_str(&text);
                            let _ = self.event_tx.send(Event::Model(ModelEvent::Chunk {
                                agent_id: self.id.clone(),
                                content: ContentChunk::Text(text),
                            })).await;
                        }
                        ModelStreamItem::Chunk(ContentChunk::Thinking { thinking, signature }) => {
                            current_thinking.push_str(&thinking);
                            let _ = self.event_tx.send(Event::Model(ModelEvent::Chunk {
                                agent_id: self.id.clone(),
                                content: ContentChunk::Thinking { thinking, signature },
                            })).await;
                        }
                        ModelStreamItem::Chunk(ContentChunk::RedactedThinking) => {
                            content_blocks.push(ContentBlock::RedactedThinking {
                                data: String::new(),
                            });
                        }
                        ModelStreamItem::ToolCall(request) => {
                            pending_tool_calls.push(ToolCall {
                                id: request.id,
                                name: request.name,
                                arguments: request.arguments,
                            });
                        }
                        ModelStreamItem::Complete => break,
                        ModelStreamItem::Fallback { from, to } => {
                            let _ = self.event_tx.send(Event::Model(ModelEvent::Fallback {
                                agent_id: self.id.clone(),
                                from,
                                to,
                            })).await;
                        }
                        ModelStreamItem::TokenUsage { prompt_tokens, completion_tokens } => {
                            // NOTE: this is right because each response's prompt_tokens will contain whole history
                            let total = prompt_tokens + completion_tokens;
                            self.pending_token_usage = Some(MessageTokenUsage {
                                prompt_tokens,
                                completion_tokens,
                                total_tokens: total,
                            });
                            // Get context window from compactor or use default
                            let context_window = self.shared.compactor.as_ref()
                                .map_or(compactor::DEFAULT_CONTEXT_WINDOW, |c| c.context_window);
                            let _ = self.event_tx.send(Event::Model(ModelEvent::TokenUsage {
                                agent_id: self.id.clone(),
                                prompt_tokens,
                                completion_tokens,
                                total_tokens: total,
                                context_window,
                            })).await;
                        }
                    },
                    Ok(None) => break,
                    Err(e) => {
                        return Err(AgentError::Provider(e));
                    }
                }
            }
        }

        if !current_thinking.is_empty() {
            content_blocks.push(ContentBlock::Thinking {
                thinking: current_thinking,
                signature: None,
            });
        }
        if !current_text.is_empty() {
            content_blocks.push(ContentBlock::Text { text: current_text });
        }

        Ok((content_blocks, pending_tool_calls))
    }

    /// Check and run compaction if needed
    async fn maybe_compact_messages(&mut self) {
        let Some(compactor) = &self.shared.compactor else {
            return; // No compactor configured, skip
        };
        let should_compact = compactor.should_compact(self.message_buffer.messages());
        if !should_compact {
            return;
        }

        // Emit start event (before borrowing self mutably for messages)
        let agent_id = self.id.clone();
        let _ = self
            .event_tx
            .send(Event::Model(ModelEvent::Compacting {
                agent_id: agent_id.clone(),
                active: true,
            }))
            .await;

        let messages = self.message_buffer.messages();
        let result = compactor
            .auto_compact(messages, &*self.shared.provider, &self.shared.model_config)
            .await;

        // Handle result and update messages
        match result {
            Ok(Some(new_messages)) => {
                let old_count = messages.len();
                let new_count = new_messages.len();
                let compacted_count = old_count.saturating_sub(new_count);
                let is_full = compacted_count > 0;

                if is_full {
                    tracing::info!(
                        "Agent {} performed full compaction: {} messages summarized",
                        agent_id,
                        compacted_count
                    );
                } else {
                    tracing::info!("Agent {} performed micro-compaction", agent_id);
                }

                // Update message buffer with compacted messages
                self.message_buffer.messages_mut().clone_from(&new_messages);

                // Persist compacted state
                if let Some(storage) = &self.shared.storage {
                    let sid = crate::types::SessionId(self.session_id.clone());
                    let messages_for_storage: Vec<Message> =
                        new_messages.iter().map(|m| (**m).clone()).collect();
                    if let Err(e) = storage.set_messages(&sid, &messages_for_storage).await {
                        tracing::warn!(
                            "Agent {} failed to persist compacted messages: {}",
                            agent_id,
                            e
                        );
                    }
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("Agent {} compaction failed: {}", agent_id, e),
        }

        // Emit end event (after match block, messages borrow is released)
        let _ = self
            .event_tx
            .send(Event::Model(ModelEvent::Compacting {
                agent_id: self.id.clone(),
                active: false,
            }))
            .await;
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
                .map_or(0, |c| c.len());
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
            let _ = self
                .event_tx
                .send(Event::Model(ModelEvent::Completed {
                    agent_id: self.id.clone(),
                }))
                .await;
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

        // Second: Check permissions for each tool call
        let mut approved_calls = Vec::new();
        let mut denied_results = Vec::new();

        for call in tool_calls {
            let level = ToolLevelResolver::resolve(&call.name, &call.arguments);

            // Check if permission is needed
            if let Some(ref checker) = self.permission_checker {
                match checker.check_permission(call, level).await {
                    Ok(true) => {
                        // Approved, add to approved calls
                        approved_calls.push(call.clone());
                    }
                    Ok(false) => {
                        // Denied, create error result
                        tracing::warn!(
                            "Agent {} tool call {} denied: {} exceeds threshold",
                            self.id,
                            call.id,
                            call.name
                        );
                        let error_msg = format!(
                            "Permission denied: {} tool (level: {:?}) was not approved by user",
                            call.name, level
                        );
                        denied_results.push(ToolExecutionResult {
                            tool_call_id: call.id.clone(),
                            event: ToolEvent::Error {
                                agent_id: self.id.clone(),
                                tool_id: call.id.clone(),
                                error: error_msg.clone(),
                                elapsed_ms: 0,
                            },
                            message: Message::tool_result(call.id.clone(), error_msg),
                        });
                    }
                    Err(e) => {
                        // Error checking permission, treat as denied
                        tracing::error!(
                            "Agent {} permission check failed for {}: {}",
                            self.id,
                            call.name,
                            e
                        );
                        let error_msg = format!("Permission check failed: {e}");
                        denied_results.push(ToolExecutionResult {
                            tool_call_id: call.id.clone(),
                            event: ToolEvent::Error {
                                agent_id: self.id.clone(),
                                tool_id: call.id.clone(),
                                error: error_msg.clone(),
                                elapsed_ms: 0,
                            },
                            message: Message::tool_result(call.id.clone(), error_msg),
                        });
                    }
                }
            } else {
                // No permission checker (YOLO mode), approve all
                approved_calls.push(call.clone());
            }
        }

        // Execute only approved calls
        let results = if approved_calls.is_empty() {
            Vec::new()
        } else {
            crate::tools::execute_tools_parallel(
                &self.id,
                &approved_calls,
                &self.tool_registry,
                Some(&self.cancel_token),
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
            .filter_map(|m| m.tool_calls.as_ref().map(|c| c.len()))
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
                    return self
                        .fail_agent("Streaming failed after max retries", e)
                        .await;
                }
                Err(e) if !Self::is_retryable_error(&e) => {
                    return self
                        .fail_agent("Streaming failed with non-retryable error", e)
                        .await;
                }
                Err(e) => {
                    attempt += 1;
                    tracing::warn!("Streaming failed (attempt {}), retrying: {}", attempt, e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(attempt)).await;
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
