use crate::agent::{is_cancelled_error, AgentShared, SimpleAgent, SubAgentMode};
use crate::event::{Event, ModelEvent, ToolEvent};
use crate::skill::Skill;
use crate::storage::SessionStore;
use crate::tools::{Tool, ToolExecCtx, ToolRegistry};
use crate::types::{AgentId, ContentBlock, KernelError, Message, Result, ToolOutput};
use crate::utils::tokens::format_tokens;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

pub const SUBAGENT_TOOL_NAME: &str = "agent";

/// Tool for spawning sub-agents to handle specific tasks
pub struct SubagentTool {
    parent_id: AgentId,
    shared: Arc<AgentShared>,
    /// Parent's `input_tx` for forwarding async sub-agent results
    parent_input_tx: mpsc::Sender<crate::agent::AgentInput>,
    /// Skills inherited from parent agent
    skills: Vec<Arc<Skill>>,
    /// Session store for creating sub-agent sessions
    session_store: Option<Arc<dyn SessionStore>>,
    /// Parent session ID for task store sharing
    parent_session_id: String,
    /// Parent's `event_tx` for forwarding permission requests and progress
    /// Subagent's permission requests and progress will be sent here so TUI can show dialogs
    parent_event_tx: mpsc::Sender<Event>,
}

impl SubagentTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent_id: AgentId,
        shared: Arc<AgentShared>,
        parent_input_tx: mpsc::Sender<crate::agent::AgentInput>,
        skills: Vec<Arc<Skill>>,
        session_store: Option<Arc<dyn SessionStore>>,
        parent_session_id: String,
        parent_event_tx: mpsc::Sender<Event>,
    ) -> Self {
        Self {
            parent_id,
            shared,
            parent_input_tx,
            skills,
            session_store,
            parent_session_id,
            parent_event_tx,
        }
    }

    /// Build the system prompt for the sub-agent
    fn build_system_prompt(&self, inherit_context: bool) -> String {
        let context_note = if inherit_context {
            "Given the conversation context provided, use the tools available to complete the task."
        } else {
            "Given the user's message, use the tools available to complete the task."
        };

        format!(
            r"You are a sub-agent of {parent_id}. {context_note}

Complete the task fully — don't gold-plate, but don't leave it half-done. When you complete the task, respond with a concise report covering what was done and any key findings — the caller will relay this to the user, so it only needs the essentials.",
            parent_id = self.parent_id,
        )
    }

    /// Create a `SimpleAgent` with the same configuration as this subagent tool
    fn create_simple_agent(&self, session_id: &str, working_dir: &std::path::Path) -> SimpleAgent {
        use crate::permissions::Checker;
        let tool_registry = self.create_tool_registry(session_id);
        let agent_id = crate::types::AgentId::new();

        // Create permission checker if permission state is available
        let permission_checker = self.shared.permission_state.as_ref().map(|state| {
            std::sync::Arc::new(Checker::new(
                state.clone(),
                agent_id.clone(),
                self.parent_event_tx.clone(),
            ))
        });

        SimpleAgent::new(
            self.shared.provider.clone(),
            (*self.shared.model_config).clone(),
            tool_registry,
            working_dir,
        )
        .with_agent_id(agent_id)
        .with_event_tx(self.parent_event_tx.clone())
        .with_permission_checker_opt(permission_checker)
    }

    /// Create tool registry for the subagent
    fn create_tool_registry(&self, session_id: &str) -> ToolRegistry {
        // Subagent doesn't need input_tx since it doesn't receive AgentInput.
        // Subagents get a fresh file state store (not shared with parent).
        crate::tools::ToolRegistryFactory::create(crate::tools::ToolRegistryConfig::for_subagent(
            &self.parent_id,
            &self.shared,
            &self.parent_event_tx,
            self.skills.clone(),
            session_id,
            &self.parent_session_id,
        ))
    }
}

/// Sub-agent completion status
#[derive(Debug)]
enum SubAgentStatus {
    Completed,
    Failed(String),
    Cancelled,
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &'static str {
        SUBAGENT_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        r#"Launch a new agent to handle complex, multi-step tasks autonomously.

## When to Use
- Research tasks requiring multiple file reads or searches
- Implementation work that requires changes across multiple files
- Complex analysis that would clutter the main context with intermediate results
- Tasks that can be parallelized for better performance

## When NOT to Use
- If you want to read a specific file path, use the read tool directly
- If you are searching for code, use the grep tool instead
- Other simple tasks requiring only 1-2 quick edits

## Parallel Execution
When you have multiple independent tasks, launch multiple agents concurrently by sending a **single message with multiple agent tool calls**. For example, if you need to audit dependencies AND refactor the auth module, send both agent calls in the same message.

## Writing the Prompt
Brief the agent like a smart colleague who just walked into the room — it hasn't seen this conversation and doesn't know what you've tried.
- Explain what you're trying to accomplish and why
- Describe what you've already learned or ruled out
- Give enough context about the surrounding problem that the agent can make judgment calls rather than just following a narrow instruction
- If you need a short response, say so ("report in under 200 words")
- Lookups: hand over the exact command. Investigations: hand over the question."#
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short summary (3-5 words) of what the agent will do, e.g., 'Audit dependencies', 'Refactor auth module'"
                },
                "prompt": {
                    "type": "string",
                    "description": "Instructions for the agent. Brief clearly - what to do, why, and expected output. Include task ID if using task tracking."
                },
                "mode": {
                    "type": "string",
                    "enum": ["async", "sync"],
                    "description": "Execution mode. 'sync' (default) waits for completion and returns results. 'async' returns immediately and runs in background — use for independent work that doesn't block your next steps.",
                    "default": "sync"
                },
                "inherit_context": {
                    "type": "boolean",
                    "description": "Give the agent access to this conversation history. Use when agent needs full context.",
                    "default": false
                }
            },
            "required": ["description", "prompt"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // Extract and clone all values from args first to avoid lifetime issues
        let description = args["description"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'description' argument"))?
            .to_string();
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'prompt' argument"))?
            .to_string();

        let mode_str = args["mode"].as_str().unwrap_or("sync");
        let mode = match mode_str {
            "async" => SubAgentMode::Async,
            _ => SubAgentMode::Sync,
        };

        let inherit_context = args["inherit_context"].as_bool().unwrap_or(false);

        tracing::info!(
            "Spawning sub-agent {} for parent {}: {} (inherit_context: {})",
            ctx.tool_call_id,
            self.parent_id,
            description,
            inherit_context
        );

        // Build system prompt (role definition only, no task specifics)
        let system_prompt = self.build_system_prompt(inherit_context);

        // Create session for transcript recording if storage is available
        let subagent_session_id = if let Some(session_store) = &self.session_store {
            // Use parent's working_dir (from ctx.working_dir) for the subagent session
            let working_dir = ctx.working_dir.to_string_lossy().to_string();
            match session_store.create(Some(&working_dir)).await {
                Ok(sid) => {
                    tracing::debug!(
                        "Created sub-agent session: {} for agent {}",
                        sid.0,
                        self.parent_id
                    );
                    sid.0
                }
                Err(e) => {
                    tracing::warn!("Failed to create sub-agent session: {}", e);
                    return Ok(ToolOutput::error(
                        "Failed to create storage session for sub-agent",
                    ));
                }
            }
        } else {
            return Ok(ToolOutput::error(
                "Storage is required to spawn sub-agents for transcript recording",
            ));
        };

        // Create SimpleAgent for execution
        let mut simple_agent = self.create_simple_agent(&subagent_session_id, &ctx.working_dir);
        let sub_agent_id = AgentId::new();

        // Prepare history if inherit_context is enabled
        let history: Option<Vec<Arc<Message>>> = if inherit_context {
            ctx.parent_messages.map(|msgs| msgs.to_vec())
        } else {
            None
        };

        // Get cancel token from context
        let cancel_token = ctx.cancel_token.clone().unwrap_or_default();

        // Execute based on mode
        match mode {
            SubAgentMode::Async => {
                // Clone values for the async block
                let parent_tx = self.parent_input_tx.clone();
                let parent_event_tx = self.parent_event_tx.clone();
                let parent_id = self.parent_id.clone();
                let desc = description.clone();
                let sub_id = sub_agent_id.clone();
                let tool_id = ctx.tool_call_id.to_string();

                // Spawn background task to execute subagent
                // Clone shared resources for the async block
                let shared = self.shared.clone();
                let parent_session_id = self.parent_session_id.clone();
                tokio::spawn(async move {
                    let (output, status) = Self::execute_simple_agent_with_shared(
                        &mut simple_agent,
                        system_prompt,
                        history,
                        prompt,
                        cancel_token,
                        &parent_event_tx,
                        &parent_id,
                        &tool_id,
                        shared,
                        parent_session_id,
                    )
                    .await;

                    // Format and send result back to parent
                    let result_text = Self::format_result_text(&desc, &sub_id, &output, &status);
                    let _ = parent_tx
                        .send(crate::agent::AgentInput::TaskResult {
                            task_id: sub_id.to_string(),
                            content: vec![ContentBlock::Text { text: result_text }],
                        })
                        .await;
                });

                let result = format!(
                    "Sub-agent '{description}' ({sub_agent_id}) spawned in async mode. Results will be sent when complete."
                );
                Ok(ToolOutput::text(result))
            }
            SubAgentMode::Sync => {
                let (output, status) = self
                    .execute_simple_agent(
                        &mut simple_agent,
                        system_prompt,
                        history,
                        prompt,
                        cancel_token,
                        &self.parent_event_tx,
                        &self.parent_id,
                        ctx.tool_call_id,
                    )
                    .await;

                info!(
                    "Sub-agent {} completed with status: {:?}",
                    sub_agent_id, status
                );

                Ok(Self::build_tool_output(output, status))
            }
        }
    }
}

impl SubagentTool {
    /// Format the result text for subagent output
    fn format_result_text(
        description: &str,
        sub_agent_id: &AgentId,
        output: &str,
        status: &SubAgentStatus,
    ) -> String {
        let final_output = match status {
            SubAgentStatus::Failed(error) => format!("{output}\n\n[Sub-agent failed: {error}]"),
            SubAgentStatus::Cancelled => format!("{output}\n\n[Sub-agent was cancelled]"),
            SubAgentStatus::Completed => output.to_string(),
        };

        let (header, section) = if matches!(status, SubAgentStatus::Completed) {
            ("Sub-agent Task Completed", "Result")
        } else {
            ("Sub-agent Task Ended (Incomplete)", "Partial Result")
        };

        format!(
            "## {header}\n\n**Task**: {description}\n**ID**: {sub_agent_id}\n\n### {section}\n{final_output}",
        )
    }

    /// Build `ToolOutput` from execution status
    fn build_tool_output(output: String, status: SubAgentStatus) -> ToolOutput {
        match status {
            SubAgentStatus::Completed => ToolOutput::text(output),
            SubAgentStatus::Failed(error) => {
                ToolOutput::error(format!("{output}\nSub-agent failed: {error}"))
            }
            SubAgentStatus::Cancelled => {
                ToolOutput::error(format!("{output}\nSub-agent was cancelled"))
            }
        }
    }

    /// Send a progress event, logging any errors
    fn send_progress(
        event_tx: &mpsc::Sender<Event>,
        agent_id: AgentId,
        tool_id: &str,
        message: String,
        tokens: Option<u32>,
    ) {
        if let Err(e) = event_tx.try_send(Event::Tool(ToolEvent::Progress {
            agent_id,
            tool_id: tool_id.to_string(),
            message,
            tokens,
        })) {
            tracing::warn!("Failed to send progress event: {}", e);
        }
    }

    /// Handle model events during execution, returning the final iteration count
    fn handle_model_event(
        event: &Event,
        iteration_count: &mut usize,
        event_tx: &mpsc::Sender<Event>,
        agent_id: AgentId,
        tool_id: &str,
    ) {
        match event {
            Event::Model(ModelEvent::TokenUsage {
                prompt_tokens,
                completion_tokens,
                ..
            }) => {
                let total = prompt_tokens + completion_tokens;
                Self::send_progress(
                    event_tx,
                    agent_id,
                    tool_id,
                    format!("iter {iteration_count} · {} tokens", format_tokens(total)),
                    Some(total),
                );
            }
            Event::Model(ModelEvent::Request { .. }) => {
                *iteration_count += 1;
                Self::send_progress(
                    event_tx,
                    agent_id,
                    tool_id,
                    format!("iteration {iteration_count} · running..."),
                    None,
                );
            }
            // Show tool calls in progress for BROWSE mode
            Event::Tool(ToolEvent::Started { tool_name, .. }) => {
                Self::send_progress(
                    event_tx,
                    agent_id,
                    tool_id,
                    format!("iteration {iteration_count} · {tool_name}"),
                    None,
                );
            }
            _ => {}
        }
    }

    /// Static helper to record token usage
    #[allow(dead_code)]
    async fn do_record_token_usage(
        _shared: Arc<AgentShared>,
        _parent_session_id: &str,
        _parent_id: &AgentId,
        _metrics: &crate::agent::ExecuteMetrics,
    ) {
        // TODO: Inject UsageStore to record subagent token usage
        // This requires architectural changes to pass UsageStore through AgentShared
    }

    /// Execute a `SimpleAgent` with shared resources (for async mode)
    #[allow(clippy::too_many_arguments)]
    async fn execute_simple_agent_with_shared(
        simple_agent: &mut SimpleAgent,
        system_prompt: String,
        history: Option<Vec<Arc<Message>>>,
        task: String,
        cancel_token: tokio_util::sync::CancellationToken,
        parent_event_tx: &mpsc::Sender<Event>,
        parent_id: &AgentId,
        tool_id: &str,
        shared: Arc<AgentShared>,
        parent_session_id: String,
    ) -> (String, SubAgentStatus) {
        let event_tx = parent_event_tx.clone();
        let agent_id = parent_id.clone();
        let tool_id_owned = tool_id.to_string();
        let mut iteration_count = 0usize;

        let result = simple_agent
            .execute(system_prompt, history, task, cancel_token, |event| {
                Self::handle_model_event(
                    &event,
                    &mut iteration_count,
                    &event_tx,
                    agent_id.clone(),
                    &tool_id_owned,
                );
            })
            .await;

        // Handle result and send final progress
        match result {
            Ok((_, metrics)) => {
                let total = metrics.token_usage.total_tokens();

                // Record token usage for subagent
                Self::do_record_token_usage(shared, &parent_session_id, parent_id, &metrics).await;

                let status = if metrics.completed {
                    Self::send_progress(
                        parent_event_tx,
                        parent_id.clone(),
                        tool_id,
                        format!("completed · {} tokens", format_tokens(total)),
                        Some(total),
                    );
                    SubAgentStatus::Completed
                } else {
                    // Max iterations reached without completing
                    Self::send_progress(
                        parent_event_tx,
                        parent_id.clone(),
                        tool_id,
                        format!("partial (max iter) · {} tokens", format_tokens(total)),
                        Some(total),
                    );
                    SubAgentStatus::Failed(format!(
                        "Task did not complete within {} iterations. \
                        Consider: 1) Breaking the task into smaller sub-tasks, \
                        or 2) Adjusting the iteration limit if needed.",
                        metrics.iteration_count
                    ))
                };
                (metrics.output_text, status)
            }
            Err(e) => {
                let error_str = e.to_string();
                let (msg, status) = if is_cancelled_error(&e) {
                    ("cancelled".to_string(), SubAgentStatus::Cancelled)
                } else {
                    (
                        format!("failed · {error_str}"),
                        SubAgentStatus::Failed(error_str),
                    )
                };
                Self::send_progress(parent_event_tx, parent_id.clone(), tool_id, msg, None);
                (String::new(), status)
            }
        }
    }

    /// Execute a `SimpleAgent` and collect output with progress events
    #[allow(clippy::too_many_arguments)]
    async fn execute_simple_agent(
        &self,
        simple_agent: &mut SimpleAgent,
        system_prompt: String,
        history: Option<Vec<Arc<Message>>>,
        task: String,
        cancel_token: tokio_util::sync::CancellationToken,
        parent_event_tx: &mpsc::Sender<Event>,
        parent_id: &AgentId,
        tool_id: &str,
    ) -> (String, SubAgentStatus) {
        Self::execute_simple_agent_with_shared(
            simple_agent,
            system_prompt,
            history,
            task,
            cancel_token,
            parent_event_tx,
            parent_id,
            tool_id,
            self.shared.clone(),
            self.parent_session_id.clone(),
        )
        .await
    }
}
