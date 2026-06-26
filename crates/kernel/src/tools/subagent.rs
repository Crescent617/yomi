use crate::agent::{is_cancelled_error, AgentShared, SimpleAgent, SubAgentMode};
use crate::event::{Event, ModelEvent, ToolEvent};
use crate::skill::Skill;
use crate::storage::SessionStore;
use crate::tools::{
    edit::EDIT_TOOL_NAME, reminder::REMINDER_TOOL_NAME, todo::TODO_TOOL_NAME,
    write::WRITE_TOOL_NAME, Tool, ToolExecCtx, ToolRegistry,
};
use crate::types::{AgentId, ContentBlock, KernelError, Message, Result, SessionId, ToolOutput};
use crate::utils::tokens::format_actual_tokens;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, Instrument};

pub const SUBAGENT_TOOL_NAME: &str = "agent";

/// Tool for spawning sub-agents to handle specific tasks
pub struct SubagentTool {
    parent_id: AgentId,
    shared: Arc<AgentShared>,
    /// Parent's `input_tx` for forwarding async sub-agent results
    parent_input_tx: mpsc::Sender<crate::agent::AgentInput>,
    /// Session store for creating sub-agent sessions
    session_store: Option<Arc<dyn SessionStore>>,
    /// Parent session ID for task store sharing
    parent_session_id: String,
    /// Parent's `event_tx` for forwarding permission requests and progress
    /// Subagent's permission requests and progress will be sent here so TUI can show dialogs
    parent_event_tx: mpsc::Sender<Event>,
    /// Tool blocklist inherited from parent agent
    tool_blocklist: Vec<String>,
}

/// Parameters for executing a sub-agent.
struct SubAgentExecParams<'a> {
    simple_agent: &'a mut SimpleAgent,
    system_prompt: String,
    history: Option<Vec<Arc<Message>>>,
    task: String,
    cancel_token: tokio_util::sync::CancellationToken,
    parent_event_tx: &'a mpsc::Sender<Event>,
    parent_id: &'a AgentId,
    tool_id: &'a str,
    shared: Arc<AgentShared>,
    parent_session_id: String,
    message_id: crate::types::MessageId,
}

impl SubagentTool {
    pub fn new(
        parent_id: AgentId,
        shared: Arc<AgentShared>,
        parent_input_tx: mpsc::Sender<crate::agent::AgentInput>,
        session_store: Option<Arc<dyn SessionStore>>,
        parent_session_id: String,
        parent_event_tx: mpsc::Sender<Event>,
        tool_blocklist: Vec<String>,
    ) -> Self {
        Self {
            parent_id,
            shared,
            parent_input_tx,
            session_store,
            parent_session_id,
            parent_event_tx,
            tool_blocklist,
        }
    }

    /// Build the system prompt for the sub-agent
    fn build_system_prompt(&self, inherit_context: bool, preset: Option<SubagentPreset>) -> String {
        let context_note = if inherit_context {
            "Given the conversation context provided, use the tools available to complete the task."
        } else {
            "Given the user's message, use the tools available to complete the task."
        };

        let mut prompt = format!(
            r"You are a sub-agent of {parent_id}. {context_note}

Complete the task fully — don't gold-plate, but don't leave it half-done. When you complete the task, respond with a concise report covering what was done and any key findings — the caller will relay this to the user, so it only needs the essentials.",
            parent_id = self.parent_id,
            context_note = context_note,
        );

        if let Some(p) = preset {
            if let Some(text) = p.prompt() {
                prompt.push_str(text);
            }
        }

        prompt
    }

    /// Create a `SimpleAgent` with the same configuration as this subagent tool
    fn create_simple_agent(
        &self,
        session_id: &str,
        working_dir: &std::path::Path,
        skills: Vec<Arc<Skill>>,
        preset: Option<SubagentPreset>,
    ) -> SimpleAgent {
        use crate::permissions::Checker;
        let agent_id = crate::types::AgentId::new();
        let mut tool_registry = self.create_tool_registry(session_id, &agent_id);

        // Remove tools disallowed by the preset
        if let Some(p) = preset {
            for tool_name in p.disallowed_tools() {
                tool_registry.remove(tool_name);
            }
        }

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
            session_id,
        )
        .with_agent_id(agent_id)
        .with_event_tx(self.parent_event_tx.clone())
        .with_permission_checker_opt(permission_checker)
        .with_skills(skills)
    }

    /// Create tool registry for the subagent
    fn create_tool_registry(&self, session_id: &str, agent_id: &AgentId) -> ToolRegistry {
        // Subagent doesn't need input_tx since it doesn't receive AgentInput.
        // Subagents get a fresh file state store (not shared with parent).
        crate::tools::ToolRegistryFactory::create(crate::tools::ToolRegistryConfig::for_subagent(
            agent_id,
            &self.shared,
            &self.parent_event_tx,
            session_id,
            self.tool_blocklist.clone(),
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
- Research requiring multiple file reads or searches
- Implementation across multiple files
- Complex analysis that would clutter context
- Tasks that can be parallelized (launch multiple agents in one message)

## When NOT to Use
- Read a specific file → use read tool
- Search code → use grep tool
- 1-2 quick edits → do them directly

## Prompt Tips
Brief the agent like a smart colleague who just walked in — it has no context.
- Explain what to do and why
- State what you've already ruled out
- Give exact commands for lookups, open-ended questions for investigations
- Set inherit_context to true when the task needs this conversation history
- Request short responses explicitly when needed ("report in under 200 words")"#
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
                },
                "preset": {
                    "type": "string",
                    "enum": ["general-purpose", "explorer", "reviewer"],
                    // Intentionally only exposing 3 presets in the schema to
                    // keep the surface area small. planner and tester work but
                    // are reserved for advanced/internal use.
                    "description": "Agent preset that configures role and available tools. 'general-purpose' (default) is the standard sub-agent. 'explorer' is read-only search specialist. 'reviewer' performs code review without editing.",
                    "default": "general-purpose"
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
        let preset = args["preset"]
            .as_str()
            .and_then(|s| s.parse::<SubagentPreset>().ok());

        if args["preset"].as_str().is_some() && preset.is_none() {
            tracing::warn!(
                "Unknown subagent preset '{}', falling back to general-purpose",
                args["preset"].as_str().unwrap_or(""),
            );
        }

        tracing::info!(
            "spawning sub-agent {} for parent {} (inherit_context: {}, preset: {:?})",
            ctx.tool_call_id,
            self.parent_id,
            inherit_context,
            preset
        );

        // Build system prompt (role definition only, no task specifics)
        let system_prompt = self.build_system_prompt(inherit_context, preset);

        // Create session for transcript recording if storage is available.
        // Reuse the pre-generated tool-call message_id as the session id so
        // that the sub-agent transcript is directly traceable from the tool
        // event stream.
        let subagent_session_id = if let Some(session_store) = &self.session_store {
            let working_dir = ctx.working_dir.to_string_lossy().to_string();
            let sid = SessionId(ctx.message_id.as_str().to_string());
            match session_store
                .create(&sid, None, Some(&working_dir), None)
                .await
            {
                Ok(()) => {
                    tracing::debug!("created sub-agent session for parent {}", self.parent_id);
                    sid.0
                }
                Err(e) => {
                    tracing::warn!("failed to create sub-agent session: {}", e);
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
        let mut simple_agent = self.create_simple_agent(
            &subagent_session_id,
            &ctx.working_dir,
            ctx.skills.clone(),
            preset,
        );
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
                let message_id = ctx.message_id.clone();
                tokio::spawn(
                    async move {
                        let (output, status) =
                            Self::execute_simple_agent_with_shared(SubAgentExecParams {
                                simple_agent: &mut simple_agent,
                                system_prompt,
                                history,
                                task: prompt,
                                cancel_token,
                                parent_event_tx: &parent_event_tx,
                                parent_id: &parent_id,
                                tool_id: &tool_id,
                                shared,
                                parent_session_id,
                                message_id,
                            })
                            .await;

                        // Format and send result back to parent
                        let result_text =
                            Self::format_result_text(&desc, &sub_id, &output, &status);
                        let _ = parent_tx
                            .send(crate::agent::AgentInput::TaskResult {
                                task_id: sub_id.to_string(),
                                content: vec![ContentBlock::Text { text: result_text }],
                            })
                            .await;
                    }
                    .instrument(tracing::Span::current()),
                );

                let result = format!(
                    "Sub-agent({sub_agent_id}) with task '{description}' spawned in async mode. Results will be sent automatically when complete."
                );
                Ok(ToolOutput::text(result))
            }
            SubAgentMode::Sync => {
                let (output, status) = Self::execute_simple_agent_with_shared(SubAgentExecParams {
                    simple_agent: &mut simple_agent,
                    system_prompt,
                    history,
                    task: prompt,
                    cancel_token,
                    parent_event_tx: &self.parent_event_tx,
                    parent_id: &self.parent_id,
                    tool_id: ctx.tool_call_id,
                    shared: self.shared.clone(),
                    parent_session_id: self.parent_session_id.clone(),
                    message_id: ctx.message_id.clone(),
                })
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
        message_id: crate::types::MessageId,
    ) {
        if let Err(e) = event_tx.try_send(Event::Tool(ToolEvent::Progress {
            agent_id,
            message_id,
            tool_id: tool_id.to_string(),
            message,
            tokens,
        })) {
            tracing::warn!("failed to send progress event: {}", e);
        }
    }

    /// Handle model events during execution, returning the final iteration count
    fn handle_model_event(
        event: &Event,
        iteration_count: &mut usize,
        event_tx: &mpsc::Sender<Event>,
        agent_id: AgentId,
        tool_id: &str,
        message_id: &crate::types::MessageId,
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
                    format!(
                        "iter {iteration_count} · {} tokens",
                        format_actual_tokens(total)
                    ),
                    Some(total),
                    message_id.clone(),
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
                    message_id.clone(),
                );
            }
            // Show tool calls in progress for BROWSE mode
            Event::Tool(ToolEvent::Start { tool_name, .. }) => {
                Self::send_progress(
                    event_tx,
                    agent_id,
                    tool_id,
                    format!("iteration {iteration_count} · {tool_name}"),
                    None,
                    message_id.clone(),
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
    async fn execute_simple_agent_with_shared(
        params: SubAgentExecParams<'_>,
    ) -> (String, SubAgentStatus) {
        let event_tx = params.parent_event_tx.clone();
        let agent_id = params.parent_id.clone();
        let tool_id_owned = params.tool_id.to_string();
        let mut iteration_count = 0usize;

        let result = params
            .simple_agent
            .execute(
                params.system_prompt,
                params.history,
                params.task,
                params.cancel_token,
                |event| {
                    Self::handle_model_event(
                        &event,
                        &mut iteration_count,
                        &event_tx,
                        agent_id.clone(),
                        &tool_id_owned,
                        &params.message_id,
                    );
                },
            )
            .await;

        // Handle result and send final progress
        match result {
            Ok((_, metrics)) => {
                let total = metrics.token_usage.total_tokens();

                // Record token usage for subagent
                Self::do_record_token_usage(
                    params.shared,
                    &params.parent_session_id,
                    params.parent_id,
                    &metrics,
                )
                .await;

                let status = if metrics.completed {
                    Self::send_progress(
                        params.parent_event_tx,
                        params.parent_id.clone(),
                        params.tool_id,
                        format!("completed · {} tokens", format_actual_tokens(total)),
                        Some(total),
                        params.message_id.clone(),
                    );
                    SubAgentStatus::Completed
                } else {
                    // Max iterations reached without completing
                    Self::send_progress(
                        params.parent_event_tx,
                        params.parent_id.clone(),
                        params.tool_id,
                        format!(
                            "partial (max iter) · {} tokens",
                            format_actual_tokens(total)
                        ),
                        Some(total),
                        params.message_id.clone(),
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
                Self::send_progress(
                    params.parent_event_tx,
                    params.parent_id.clone(),
                    params.tool_id,
                    msg,
                    None,
                    params.message_id.clone(),
                );
                (String::new(), status)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentPreset {
    /// Default sub-agent — full toolkit, generic instructions.
    GeneralPurpose,
    /// Read-only codebase exploration specialist. Fast, parallel searches.
    Explorer,
    /// Code review specialist — examines changes for correctness, security,
    /// performance and maintainability without editing files.
    Reviewer,
    /// Architecture planner — explores existing code and produces step-by-step
    /// implementation plans.
    Planner,
    /// Verification specialist — runs builds, tests, and adversarial probes.
    /// May write ephemeral scripts outside the project directory.
    Tester,
}

impl std::str::FromStr for SubagentPreset {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // Avoid allocating via to_lowercase() by matching case-insensitively.
        match s {
            s if s.eq_ignore_ascii_case("general-purpose")
                || s.eq_ignore_ascii_case("general_purpose")
                || s.eq_ignore_ascii_case("default") =>
            {
                Ok(Self::GeneralPurpose)
            }
            s if s.eq_ignore_ascii_case("explorer") || s.eq_ignore_ascii_case("explore") => {
                Ok(Self::Explorer)
            }
            s if s.eq_ignore_ascii_case("reviewer") || s.eq_ignore_ascii_case("review") => {
                Ok(Self::Reviewer)
            }
            s if s.eq_ignore_ascii_case("planner") || s.eq_ignore_ascii_case("plan") => {
                Ok(Self::Planner)
            }
            s if s.eq_ignore_ascii_case("tester")
                || s.eq_ignore_ascii_case("test")
                || s.eq_ignore_ascii_case("verification") =>
            {
                Ok(Self::Tester)
            }
            _ => std::result::Result::Err(()),
        }
    }
}

impl SubagentPreset {
    /// Returns the text to append to the sub-agent's base system prompt,
    /// or `None` for the default preset.
    pub fn prompt(&self) -> Option<&'static str> {
        match self {
            Self::GeneralPurpose => None,
            Self::Explorer => Some(EXPLORER_PROMPT),
            Self::Reviewer => Some(REVIEWER_PROMPT),
            Self::Planner => Some(PLANNER_PROMPT),
            Self::Tester => Some(TESTER_PROMPT),
        }
    }

    /// Tool names that should be removed from the sub-agent's registry for
    /// this preset.
    pub fn disallowed_tools(&self) -> &'static [&'static str] {
        match self {
            Self::GeneralPurpose => &[],
            Self::Explorer | Self::Reviewer | Self::Planner | Self::Tester => &[
                WRITE_TOOL_NAME,
                EDIT_TOOL_NAME,
                SUBAGENT_TOOL_NAME,
                TODO_TOOL_NAME,
                REMINDER_TOOL_NAME,
            ],
        }
    }
}

static EXPLORER_PROMPT: &str = r"
# Role: Read-Only Exploration Specialist

Your role is EXCLUSIVELY to search and analyze existing code.

STRICT PROHIBITIONS:
- Do NOT create, modify, or delete any files.
- Do NOT use shell commands that change system state (no git add/commit, no mkdir/rm/touch/cp/mv in the project, no install commands).
- Do NOT run redirects (>, >>) or heredocs that write files.

Guidelines:
- Search broadly and efficiently. Use multiple parallel searches when possible.
- Read code carefully to understand patterns and architecture.
- Report findings concisely with file paths and key insights.
";

static REVIEWER_PROMPT: &str = r"
# Role: Code Review Specialist

Your job is to critically examine code and provide actionable feedback.
You do NOT modify files — your output is a review report only.

Focus areas:
- Correctness: logic errors, edge cases, off-by-one, race conditions
- Security: injection risks, unsafe operations, secret leakage
- Performance: unnecessary allocations, O(n²) patterns, blocking I/O
- Maintainability: readability, naming, test coverage, documentation
- Consistency: adherence to project conventions and existing patterns

STRICT PROHIBITIONS:
- Do NOT modify any files.
- Do NOT create files.
";

static PLANNER_PROMPT: &str = r"
# Role: Software Architect & Planning Specialist

Your role is to explore the codebase and design implementation plans.

STRICT PROHIBITIONS:
- Do NOT create, modify, or delete any files.
- Do NOT use shell commands that change system state in the project.

Your Process:
1. Understand the requirements provided in the user's message.
2. Explore thoroughly: find existing patterns, conventions, and similar features.
3. Design a solution that follows existing architecture.
4. Output a step-by-step implementation plan.
5. Identify 3-5 critical files for implementation.

REMEMBER: You can ONLY explore and plan. You CANNOT write, edit, or modify files.
";

static TESTER_PROMPT: &str = r"
# Role: Verification & Testing Specialist

Your job is to verify that implementations are correct by trying to break them.

STRICT PROHIBITIONS on the PROJECT DIRECTORY:
- Do NOT create, modify, or delete files IN the project directory.
- Do NOT run git write operations (add, commit, push).

You MAY write ephemeral test scripts to /tmp or $TMPDIR when inline commands
are insufficient. Clean up after yourself.

Verification Strategy:
1. Read project docs (README, CLAUDE.md, package.json, Makefile, etc.) for build/test commands.
2. Run the build. A broken build is automatic FAIL.
3. Run the test suite. Failing tests are automatic FAIL.
4. Run linters / type-checkers if configured.
5. Apply adversarial probes: boundary values, concurrency, invalid inputs.
6. Check for regressions in related code.

OUTPUT FORMAT:
End with exactly one of:
VERDICT: PASS
VERDICT: FAIL
VERDICT: PARTIAL

For each check, show the exact command run and the actual output observed.
";
