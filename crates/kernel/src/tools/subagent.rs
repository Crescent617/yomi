use crate::agent::{is_cancelled_error, AgentShared, SimpleAgent, SubAgentMode};
use crate::event::{Event, ModelEvent, ToolEvent};
use crate::skill::Skill;
use crate::storage::Storage;
use crate::tools::{Tool, ToolExecCtx, ToolRegistry};
use crate::types::{AgentId, ContentBlock, Message, ToolOutput};
use crate::utils::tokens::format_tokens;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

pub const SUBAGENT_TOOL_NAME: &str = "subagent";

/// Tool for spawning sub-agents to handle specific tasks
pub struct SubagentTool {
    parent_id: AgentId,
    shared: Arc<AgentShared>,
    /// Parent's `input_tx` for forwarding async sub-agent results
    parent_input_tx: mpsc::Sender<crate::agent::AgentInput>,
    /// Skills inherited from parent agent
    skills: Vec<Arc<Skill>>,
    /// Storage for transcript recording (optional)
    storage: Option<Arc<dyn Storage>>,
    /// Working directory for sub-agent
    working_dir: std::path::PathBuf,
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
        storage: Option<Arc<dyn Storage>>,
        working_dir: impl Into<std::path::PathBuf>,
        parent_session_id: String,
        parent_event_tx: mpsc::Sender<Event>,
    ) -> Self {
        Self {
            parent_id,
            shared,
            parent_input_tx,
            skills,
            storage,
            working_dir: working_dir.into(),
            parent_session_id,
            parent_event_tx,
        }
    }

    /// Build the system prompt for the sub-agent
    fn build_system_prompt(&self, inherit_context: bool) -> String {
        let context_note = if inherit_context {
            "You have been provided with the full conversation context from the parent agent, so you understand the ongoing discussion and can build upon previous work.\n- You have access to the parent's conversation history - use it to understand the full context"
        } else {
            "You have zero context about the parent conversation - rely on the user message for complete task information."
        };

        format!(
            r"You are a sub-agent spawned by parent agent {parent_id}.

## Your Role
You are a specialist agent handling a specific task delegated by the parent agent. {context_note}

## Guidelines
- Focus on the specific task described in the user message
- If the task involves code changes, read the relevant files first
- Report your findings concisely; avoid unnecessary verbosity
- When complete, provide a clear summary of what you found or accomplished
- Do NOT make assumptions about files or code you haven't examined

## Output Format
1. **Summary**: Brief overview of what you did
2. **Details**: Specific findings, changes, or results
3. **Recommendations**: Any follow-up actions needed (if applicable)
",
            parent_id = self.parent_id,
        )
    }

    /// Create a `SimpleAgent` with the same configuration as this subagent tool
    fn create_simple_agent(&self, session_id: &str) -> SimpleAgent {
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
        )
        .with_agent_id(agent_id)
        .with_event_tx(self.parent_event_tx.clone())
        .with_permission_checker_opt(permission_checker)
    }

    /// Create tool registry for the subagent
    fn create_tool_registry(&self, session_id: &str) -> ToolRegistry {
        // Subagent doesn't need input_tx since it doesn't receive AgentInput.
        // BashTool's async mode will fail gracefully with a clear error message.
        crate::tools::ToolRegistryFactory::create(
            &self.parent_id,
            &self.shared,
            &self.working_dir,
            None, // No input_tx for subagent
            &self.parent_event_tx,
            self.skills.clone(),
            session_id,
            Some(&self.parent_session_id),
            false, // Disable nested subagents to prevent infinite recursion
        )
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
- Tasks that can be parallelized for better performance
- Complex analysis that would clutter the main context with intermediate results

## When NOT to Use
- Simple file reads - use the read tool directly
- Single grep/search operations - use bash or search tools
- Tasks requiring only 1-2 quick edits

## Writing the Prompt
Brief the agent like a smart colleague who just walked into the room:
- Explain what you're trying to accomplish and why
- Describe what you've already learned or ruled out
- Give enough context for the agent to make judgment calls
- If you need a short response, say so ("report in under 200 words")
- Lookups: hand over the exact command
- Investigations: hand over the question

## Never Delegate Understanding
Don't write "based on your findings, fix the bug" - write prompts that prove YOU understood. Include file paths, line numbers, what specifically to change.

## Execution Modes
- **sync** (default and most of cases): Wait for sub-agent completion, returns full results
- **async**: Returns immediately, results sent as background notification when ready. Use async when you have genuinely independent work to do in parallel."#
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "Short summary (3-5 words) of what the sub-agent will do, e.g., 'Audit dependencies', 'Refactor auth module'"
                },
                "task": {
                    "type": "string",
                    "description": "The specific task description for the sub-agent. Be detailed - the sub-agent has no context about your conversation."
                },
                "mode": {
                    "type": "string",
                    "enum": ["async", "sync"],
                    "description": "Execution mode: 'async' returns immediately with sub-agent ID (use for parallel work), 'sync' waits for completion and returns results (use when you need the results to proceed)",
                    "default": "sync"
                },
                "inherit_context": {
                    "type": "boolean",
                    "description": "Whether to inherit the parent agent's conversation context. When true, the sub-agent will receive the parent's message history (excluding system messages). Useful when the sub-agent needs full context of the conversation.",
                    "default": false
                }
            },
            "required": ["description", "task"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // Extract and clone all values from args first to avoid lifetime issues
        let description = args["description"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'description' argument"))?
            .to_string();
        let task = args["task"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'task' argument"))?
            .to_string();

        let mode_str = args["mode"].as_str().unwrap_or("sync");
        let mode = match mode_str {
            "async" => SubAgentMode::Async,
            _ => SubAgentMode::Sync,
        };

        let inherit_context = args["inherit_context"].as_bool().unwrap_or(false);

        tracing::info!(
            "Spawning sub-agent {} for parent {} with task: {} (inherit_context: {})",
            ctx.tool_call_id,
            self.parent_id,
            description,
            inherit_context
        );

        // Build system prompt (role definition only, no task specifics)
        let system_prompt = self.build_system_prompt(inherit_context);

        // Create session for transcript recording if storage is available
        let subagent_session_id = if let Some(storage) = &self.storage {
            match storage.create_session().await {
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
                    return Ok(ToolOutput::new_err(
                        "Failed to create storage session for sub-agent",
                    ));
                }
            }
        } else {
            return Ok(ToolOutput::new_err(
                "Storage is required to spawn sub-agents for transcript recording",
            ));
        };

        // Create SimpleAgent for execution
        let mut simple_agent = self.create_simple_agent(&subagent_session_id);
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
                tokio::spawn(async move {
                    let (output, status) = Self::execute_simple_agent(
                        &mut simple_agent,
                        system_prompt,
                        history,
                        task,
                        cancel_token,
                        &parent_event_tx,
                        &parent_id,
                        &tool_id,
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
                Ok(ToolOutput {
                    stdout: result,
                    stderr: String::new(),
                    exit_code: 0,
                })
            }
            SubAgentMode::Sync => {
                let (output, status) = Self::execute_simple_agent(
                    &mut simple_agent,
                    system_prompt,
                    history,
                    task,
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
            SubAgentStatus::Completed => ToolOutput {
                stdout: output,
                stderr: String::new(),
                exit_code: 0,
            },
            SubAgentStatus::Failed(error) => ToolOutput {
                stdout: output,
                stderr: format!("Sub-agent failed: {error}"),
                exit_code: 1,
            },
            SubAgentStatus::Cancelled => ToolOutput {
                stdout: output,
                stderr: "Sub-agent was cancelled".to_string(),
                exit_code: 1,
            },
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
                    format!("iteration {iteration_count}/20 · streaming"),
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

    /// Execute a `SimpleAgent` and collect output with progress events
    #[allow(clippy::too_many_arguments)]
    async fn execute_simple_agent(
        simple_agent: &mut SimpleAgent,
        system_prompt: String,
        history: Option<Vec<Arc<Message>>>,
        task: String,
        cancel_token: tokio_util::sync::CancellationToken,
        parent_event_tx: &mpsc::Sender<Event>,
        parent_id: &AgentId,
        tool_id: &str,
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
                let total = metrics.total_prompt_tokens + metrics.total_completion_tokens;
                Self::send_progress(
                    parent_event_tx,
                    parent_id.clone(),
                    tool_id,
                    format!("completed · {} tokens", format_tokens(total)),
                    Some(total),
                );
                (metrics.output_text, SubAgentStatus::Completed)
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
}
