use crate::agent::{AgentShared, SubAgentMode};
use crate::comms::InputBus;
use crate::event::{AgentEvent, AgentStatus, Event, StopReason};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{ContentBlock, KernelError, Message, Result, SessionId, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

pub const SUBAGENT_TOOL_NAME: &str = "agent";

/// Tool for spawning sub-agents to handle specific tasks
#[derive(Clone)]
pub struct SubagentTool {
    shared: Arc<AgentShared>,
    /// `InputBus` for forwarding async sub-agent results
    input_bus: Arc<InputBus>,
    /// Parent session ID for task store sharing
    parent_session_id: SessionId,
}

/// Parameters for executing a sub-agent via `run_subagent`.
struct RunSubagentParams {
    session_id: SessionId,
    prompt: String,
    preset_text: Option<String>,
    history: Option<Vec<Arc<Message>>>,
}

impl SubagentTool {
    pub fn new(
        shared: Arc<AgentShared>,
        input_bus: Arc<InputBus>,
        parent_session_id: SessionId,
    ) -> Self {
        Self {
            shared,
            input_bus,
            parent_session_id,
        }
    }

    /// Run a sub-agent and wait for it to finish via the event bus.
    /// No intermediate progress is collected; the final output is read from the message store.
    async fn run_subagent(&self, params: RunSubagentParams) -> (String, SubAgentStatus) {
        let RunSubagentParams {
            session_id,
            prompt,
            preset_text,
            history,
        } = params;

        let session_id_str = session_id.as_str();

        // If context inheritance is enabled, copy parent history into the
        // subagent session so the Conductor can pick it up when spawning.
        if let Some(ref store) = self.shared.message_store {
            if let Some(ref hist) = history {
                let msgs: Vec<Message> = hist.iter().map(|m| (**m).clone()).collect();
                if !msgs.is_empty() {
                    // Only seed history if the subagent session is empty to avoid overwriting
                    match store.get(session_id_str).await {
                        Ok(existing) if existing.is_empty() => {
                            if let Err(e) = store.append(session_id_str, &msgs).await {
                                tracing::warn!("failed to copy history to subagent: {}", e);
                            }
                        }
                        Ok(_) => tracing::debug!(
                            "subagent session {} already has history, skipping seed",
                            session_id_str
                        ),
                        Err(e) => tracing::warn!("failed to check subagent history: {}", e),
                    }
                }
            }
        }

        // Merge preset + task prompt into a single user message sent via the input bus.
        let user_text = match preset_text {
            Some(preset) => format!("{preset}\n\n{prompt}"),
            None => prompt,
        };

        // Subscribe to the event bus and wait for the subagent to finish.
        let event_bus = self
            .shared
            .event_bus
            .as_ref()
            .expect("event_bus must be configured");
        let mut subscriber = event_bus.subscribe(session_id.clone());

        // Send the task to the subagent session via the input bus.
        // The Conductor will create the agent if it doesn't exist yet.
        if let Err(e) = self.input_bus.publish(
            session_id.clone(),
            crate::agent::AgentInput::User {
                content: vec![ContentBlock::Text { text: user_text }],
            },
        ) {
            tracing::warn!("Failed to publish subagent input: {}", e);
            return (
                format!("Failed to queue subagent task: {e}"),
                SubAgentStatus::Failed(format!("InputBus full: {e}")),
            );
        }

        let mut status = SubAgentStatus::Completed;

        while let Some((sid, event)) = subscriber.recv().await {
            if sid.0 != session_id.0 {
                continue;
            }

            if let Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Stopped { reason },
                ..
            }) = event
            {
                match &reason {
                    StopReason::Completed { .. } => {
                        status = SubAgentStatus::Completed;
                        break;
                    }
                    StopReason::Cancelled { .. } => {
                        status = SubAgentStatus::Cancelled;
                        break;
                    }
                    StopReason::Failed { error } => {
                        status = SubAgentStatus::Failed(error.clone());
                        break;
                    }
                    StopReason::MaxIterations { .. } => {
                        status = SubAgentStatus::Failed(
                            "Task did not complete within the allowed iterations. \
                             Consider: 1) Breaking the task into smaller sub-tasks, \
                             or 2) Adjusting the iteration limit if needed."
                                .to_string(),
                        );
                        break;
                    }
                }
            }
        }

        // Fetch final output from the subagent's message store
        let output_text = if let Some(ref store) = self.shared.message_store {
            match store.get(session_id_str).await {
                Ok(msgs) => msgs
                    .iter()
                    .rev()
                    .find(|m| m.role == crate::types::Role::Assistant)
                    .map(|m| {
                        m.content
                            .iter()
                            .filter_map(|b| b.as_text())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
                Err(e) => {
                    tracing::warn!("failed to get subagent final output: {}", e);
                    String::new()
                }
            }
        } else {
            String::new()
        };

        (output_text, status)
    }
}

/// Preset roles for sub-agents.
#[derive(Debug)]
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
- Tasks that can be parallelized — call this tool multiple times in one response to launch independent subagents concurrently

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
                    "description": format!("Execution mode. 'sync' (default) waits for completion and returns results. 'async' returns immediately and runs in background. {} The subagent result will be delivered as a new message automatically.", crate::tools::ASYNC_LAUNCH_GUIDE),
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

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
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
            "spawning sub-agent (inherit_context: {}, preset: {:?})",
            inherit_context,
            preset
        );

        // Prevent recursive spawning
        if self.parent_session_id.starts_with(crate::types::SUB_PREFIX) {
            return Ok(ToolOutput::error(
                "Sub-agents cannot spawn other sub-agents. Use the parent agent to coordinate multiple tasks.",
            ));
        }

        // Create subagent session ID (child of parent session)
        let session_id = SessionId::new_subagent();
        let prompt_clone = prompt.clone();

        // Emit metadata event immediately so UI can show jump link before subagent finishes
        if let Some(ref bus) = self.shared.event_bus {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("subagent_session_id".to_string(), session_id.to_string());
            metadata.insert("subagent_description".to_string(), description.clone());
            metadata.insert(
                "parent_session_id".to_string(),
                self.parent_session_id.to_string(),
            );
            if let Err(e) = bus.publish(
                self.parent_session_id.clone(),
                crate::event::Event::Tool(crate::event::ToolEvent::Metadata {
                    message_id: _ctx.message_id.clone(),
                    tool_id: _ctx.tool_call_id.to_string(),
                    metadata,
                }),
            ) {
                tracing::warn!("Failed to publish subagent metadata: {}", e);
            }
        }

        // Persist subagent session to database with parent_id and default title
        if let Some(ref store) = self.shared.session_store {
            if let Err(e) = store
                .create(
                    &session_id,
                    None, // project_id
                    None, // working_dir
                    None, // auto_approve_level
                    Some(&self.parent_session_id),
                )
                .await
            {
                tracing::warn!("failed to create subagent session record: {}", e);
            } else if let Err(e) = store.update_title(&session_id, &description).await {
                tracing::warn!("failed to set subagent session title: {}", e);
            }
        }

        // Get conversation history for subagent if context inheritance is enabled
        let history = if inherit_context {
            if let Some(ref store) = self.shared.message_store {
                match store.get(&self.parent_session_id).await {
                    Ok(msgs) => Some(msgs.into_iter().map(Arc::new).collect()),
                    Err(e) => {
                        tracing::warn!("failed to get history for subagent: {}", e);
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // Extract preset instructions as text to be merged into the user message
        let preset_text = preset
            .as_ref()
            .and_then(|p| p.prompt().map(|s| s.to_string()));

        let params = RunSubagentParams {
            session_id: session_id.clone(),
            prompt: prompt_clone,
            preset_text,
            history,
        };

        let result = match mode {
            SubAgentMode::Async => {
                let result_text = format!(
                    "Subagent with task '{description}' spawned in async mode. {} Results will be sent automatically when complete.",
                    crate::tools::ASYNC_LAUNCH_GUIDE
                );
                let self_clone = self.clone();
                tokio::spawn(async move {
                    let (output, status) = self_clone.run_subagent(params).await;
                    let steer = match &status {
                        SubAgentStatus::Completed => {
                            format!("Task '{description}' completed:\n\n{output}")
                        }
                        SubAgentStatus::Failed(e) => {
                            format!("Task '{description}' failed:\n\n{output}\n\n[error: {e}]")
                        }
                        SubAgentStatus::Cancelled => {
                            format!("Task '{description}' cancelled:\n\n{output}\n\n[cancelled]")
                        }
                    };
                    if let Err(e) = self_clone.input_bus.publish(
                        self_clone.parent_session_id.clone(),
                        crate::agent::AgentInput::Steer(vec![ContentBlock::Text { text: steer }]),
                    ) {
                        tracing::warn!("Failed to publish subagent async result: {}", e);
                    }
                });

                ToolOutput::text(result_text)
            }
            SubAgentMode::Sync => {
                let (output, status) = self.run_subagent(params).await;
                match &status {
                    SubAgentStatus::Completed => ToolOutput::text(output),
                    SubAgentStatus::Failed(e) => {
                        if output.is_empty() {
                            ToolOutput::error(e.clone())
                        } else {
                            ToolOutput::text(format!("{output}\n\n[error: {e}]"))
                        }
                    }
                    SubAgentStatus::Cancelled => {
                        if output.is_empty() {
                            ToolOutput::text("cancelled".to_string())
                        } else {
                            ToolOutput::text(format!("{output}\n\n[cancelled]"))
                        }
                    }
                }
            }
        };

        Ok(result)
    }
}
