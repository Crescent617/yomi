use crate::agent::{Agent, AgentInput, AgentShared, AgentSpawnArgs, CancelToken, SubAgentMode};
use crate::event::Event;
use crate::skill::Skill;
use crate::storage::Storage;
use crate::tools::Tool;
use crate::types::{AgentId, ContentBlock, ToolOutput};
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
    parent_input_tx: mpsc::Sender<AgentInput>,
    /// Skills inherited from parent agent
    skills: Vec<Arc<Skill>>,
    /// Storage for transcript recording (optional)
    storage: Option<Arc<dyn Storage>>,
    /// Working directory for sub-agent
    working_dir: std::path::PathBuf,
    /// Parent session ID for task store sharing
    parent_session_id: String,
    /// Parent's `event_tx` for forwarding permission requests
    /// Subagent's permission requests will be sent here so TUI can show dialogs
    parent_event_tx: mpsc::Sender<crate::event::Event>,
    /// Optional cancel token to share with parent (for cascading cancellation)
    cancel_token: Option<CancelToken>,
}

impl SubagentTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent_id: AgentId,
        shared: Arc<AgentShared>,
        parent_input_tx: mpsc::Sender<AgentInput>,
        skills: Vec<Arc<Skill>>,
        storage: Option<Arc<dyn Storage>>,
        working_dir: impl Into<std::path::PathBuf>,
        parent_session_id: String,
        parent_event_tx: mpsc::Sender<crate::event::Event>,
        cancel_token: Option<CancelToken>,
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
            cancel_token,
        }
    }

    /// Build the system prompt for the sub-agent
    fn build_system_prompt(&self) -> String {
        format!(
            r"You are a sub-agent spawned by parent agent {parent_id}.

## Your Role
You are a specialist agent handling a specific task delegated by the parent agent. You have zero context about the parent conversation - rely on the user message for complete task information.

## Guidelines
- Focus solely on the task described in the user message
- If the task involves code changes, read the relevant files first
- Report your findings concisely; avoid unnecessary verbosity
- If you need more information, use the available tools to gather it
- When complete, provide a clear summary of what you found or accomplished
- Do NOT make assumptions about files or code you haven't examined

## Output Format
Provide your response in a structured format:
1. **Summary**: Brief overview of what you did
2. **Details**: Specific findings, changes, or results
3. **Recommendations**: Any follow-up actions needed (if applicable)
",
            parent_id = self.parent_id,
        )
    }

    /// Collect output from sub-agent events
    /// Returns (output, status) where status indicates completion state
    async fn collect_subagent_output(
        event_rx: &mut mpsc::Receiver<Event>,
        output: &mut String,
        parent_event_tx: &mpsc::Sender<Event>,
        parent_id: &AgentId,
        tool_id: &str,
    ) -> SubAgentStatus {
        use crate::event::{AgentEvent, ContentChunk, ModelEvent, ToolEvent};

        // Track token usage (TokenUsage reports cumulative totals)
        let mut total_prompt_tokens: u32 = 0;
        let mut total_completion_tokens: u32 = 0;
        let mut iteration_count: usize = 0;

        while let Some(event) = event_rx.recv().await {
            match &event {
                Event::Model(ModelEvent::Chunk {
                    content: ContentChunk::Text(text),
                    ..
                }) => {
                    // Only capture text output to avoid bloating parent context
                    output.push_str(text);
                }
                Event::Model(ModelEvent::TokenUsage {
                    prompt_tokens,
                    completion_tokens,
                    ..
                }) => {
                    // TokenUsage reports cumulative totals, save latest values
                    total_prompt_tokens = *prompt_tokens;
                    total_completion_tokens = *completion_tokens;
                    let total = total_prompt_tokens + total_completion_tokens;

                    // Send progress update with token count
                    let progress_msg =
                        format!("iter {iteration_count} · {} tokens", format_tokens(total));
                    let _ = parent_event_tx
                        .send(Event::Tool(ToolEvent::Progress {
                            agent_id: parent_id.clone(),
                            tool_id: tool_id.to_string(),
                            message: progress_msg,
                            tokens: Some(total),
                        }))
                        .await;
                }
                Event::Model(ModelEvent::Request { .. }) => {
                    iteration_count += 1;
                    let progress_msg = format!("iteration {iteration_count}/20 · streaming");
                    let _ = parent_event_tx
                        .send(Event::Tool(ToolEvent::Progress {
                            agent_id: parent_id.clone(),
                            tool_id: tool_id.to_string(),
                            message: progress_msg,
                            tokens: None,
                        }))
                        .await;
                }
                Event::Agent(AgentEvent::Completed { .. }) => {
                    // Send final progress with total tokens
                    let total = total_prompt_tokens + total_completion_tokens;
                    let progress_msg = format!("completed · {} tokens", format_tokens(total));
                    let _ = parent_event_tx
                        .send(Event::Tool(ToolEvent::Progress {
                            agent_id: parent_id.clone(),
                            tool_id: tool_id.to_string(),
                            message: progress_msg,
                            tokens: Some(total),
                        }))
                        .await;
                    info!("Sub-agent completed with output");
                    return SubAgentStatus::Completed;
                }
                Event::Agent(AgentEvent::Failed { error, .. }) => {
                    let total = total_prompt_tokens + total_completion_tokens;
                    let _ = parent_event_tx
                        .send(Event::Tool(ToolEvent::Progress {
                            agent_id: parent_id.clone(),
                            tool_id: tool_id.to_string(),
                            message: format!("failed · {} tokens", format_tokens(total)),
                            tokens: Some(total),
                        }))
                        .await;
                    return SubAgentStatus::Failed(error.clone());
                }
                Event::Agent(AgentEvent::Cancelled { .. }) => {
                    let total = total_prompt_tokens + total_completion_tokens;
                    let _ = parent_event_tx
                        .send(Event::Tool(ToolEvent::Progress {
                            agent_id: parent_id.clone(),
                            tool_id: tool_id.to_string(),
                            message: format!("cancelled · {} tokens", format_tokens(total)),
                            tokens: Some(total),
                        }))
                        .await;
                    return SubAgentStatus::Cancelled;
                }
                _ => {}
            }
        }
        SubAgentStatus::Disconnected
    }
}

/// Sub-agent completion status
#[derive(Debug)]
enum SubAgentStatus {
    Completed,
    Failed(String),
    Cancelled,
    Disconnected,
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
                }
            },
            "required": ["description", "task"]
        })
    }

    async fn exec(&self, args: Value) -> Result<ToolOutput> {
        // Default: use a placeholder tool_id (subagent's own id will be generated)
        self.exec_with_id(args, "").await
    }

    async fn exec_with_id(&self, args: Value, tool_call_id: &str) -> Result<ToolOutput> {
        let description = args["description"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'description' argument"))?;
        let task = args["task"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'task' argument"))?;

        let mode_str = args["mode"].as_str().unwrap_or("sync");
        let mode = match mode_str {
            "async" => SubAgentMode::Async,
            _ => SubAgentMode::Sync,
        };

        tracing::info!(
            "Spawning sub-agent {} for parent {} with task: {}",
            tool_call_id,
            self.parent_id,
            description
        );

        // Build system prompt (role definition only, no task specifics)
        let system_prompt = self.build_system_prompt();

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

        let config = AgentSpawnArgs::new(system_prompt, subagent_session_id)
            .with_skills(self.skills.clone())
            .with_parent_session(&self.parent_session_id)
            .with_max_iterations(20)
            .without_sub_agents()
            .with_working_dir(self.working_dir.clone())
            .with_parent_event_tx(self.parent_event_tx.clone())
            .with_cancel_token(self.cancel_token.clone().unwrap_or_default());

        let (handle, mut event_rx) = Agent::spawn(AgentId::new(), &self.shared, config);

        let sub_agent_id = handle.id.clone();

        // Send the task as the first user message
        // Pattern: system prompt defines role, user message provides task
        handle.send_text(task.to_string()).await.ok();
        // Close the agent gracefully after collecting output
        let _ = handle.close().await;
        match mode {
            SubAgentMode::Async => {
                // Spawn background task to collect results and forward to parent
                let parent_tx = self.parent_input_tx.clone();
                let parent_event_tx = self.parent_event_tx.clone();
                let parent_id = self.parent_id.clone();
                let sub_id = sub_agent_id.clone();
                let desc = description.to_string();
                let tool_id = tool_call_id.to_string();
                tokio::spawn(async move {
                    let mut output = String::new();
                    let status = Self::collect_subagent_output(
                        &mut event_rx,
                        &mut output,
                        &parent_event_tx,
                        &parent_id,
                        &tool_id,
                    )
                    .await;

                    // Append error/cancelled markers to output
                    match &status {
                        SubAgentStatus::Failed(error) => {
                            use std::fmt::Write;
                            let _ = write!(output, "\n\n[Sub-agent failed: {error}]");
                        }
                        SubAgentStatus::Cancelled => {
                            output.push_str("\n\n[Sub-agent was cancelled]");
                        }
                        _ => {}
                    }

                    // Forward result to parent agent with structured format
                    let completed = matches!(status, SubAgentStatus::Completed);
                    let result_text = if completed {
                        format!(
                            "## Sub-agent Task Completed\n\n**Task**: {desc}\n**ID**: {sub_id}\n\n### Result\n{output}",
                        )
                    } else {
                        format!(
                            "## Sub-agent Task Ended (Incomplete)\n\n**Task**: {desc}\n**ID**: {sub_id}\n\n### Partial Result\n{output}"
                        )
                    };

                    // Send result back to parent via input_tx (as ContentBlock array)
                    let _ = parent_tx
                        .send(AgentInput::TaskResult {
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
                // Collect output from sub-agent
                let mut output = String::new();
                let status = Self::collect_subagent_output(
                    &mut event_rx,
                    &mut output,
                    &self.parent_event_tx,
                    &self.parent_id,
                    tool_call_id,
                )
                .await;
                info!(
                    "Sub-agent {} completed with status: {:?}",
                    sub_agent_id, status
                );

                match status {
                    SubAgentStatus::Completed => Ok(ToolOutput {
                        stdout: output,
                        stderr: String::new(),
                        exit_code: 0,
                    }),
                    SubAgentStatus::Failed(error) => Ok(ToolOutput {
                        stdout: output,
                        stderr: format!("Sub-agent failed: {error}"),
                        exit_code: 1,
                    }),
                    SubAgentStatus::Cancelled => Ok(ToolOutput {
                        stdout: output,
                        stderr: "Sub-agent was cancelled".to_string(),
                        exit_code: 1,
                    }),
                    SubAgentStatus::Disconnected => Ok(ToolOutput {
                        stdout: output,
                        stderr: "Sub-agent ended without completion".to_string(),
                        exit_code: 1,
                    }),
                }
            }
        }
    }
}
