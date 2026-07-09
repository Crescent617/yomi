use crate::agent::{AgentShared, SubAgentMode};
use crate::comms::InputBus;
use crate::event::{AgentEvent, AgentStatus, Event, StopReason};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{ContentBlock, EventId, KernelError, Result, SessionId, ToolOutput};
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
}

/// Prefix `text` with the subagent session id so callers can correlate results.
fn agent_prefix(sid: &SessionId, text: impl std::fmt::Display) -> String {
    format!("[agent_id: {sid}]\n{text}")
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
        let RunSubagentParams { session_id, prompt } = params;

        let session_id_str = session_id.as_str();

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
            crate::agent::AgentInput::Steer(vec![ContentBlock::Text { text: prompt }]),
        ) {
            tracing::warn!("Failed to publish subagent input: {}", e);
            return (
                format!("Failed to queue subagent task: {e}"),
                SubAgentStatus::Failed(format!("InputBus full: {e}")),
            );
        }

        let mut status = SubAgentStatus::Completed;

        while let Some((sid, envelope)) = subscriber.recv().await {
            if sid.0 != session_id.0 {
                continue;
            }
            let event = envelope.event;

            // Forward ask_user and permission events to the parent session so
            // UI layers (TUI/GUI) can display them in the same event stream.
            // Also forward acks so the parent can clean up stale pending requests.
            match &event {
                Event::Agent(AgentEvent::AskUserQuestion {
                    req_id, questions, ..
                }) => {
                    if let Some(ref bus) = self.shared.event_bus {
                        let _ = bus.publish(
                            self.parent_session_id.clone(),
                            crate::event::Envelope {
                                session_id: self.parent_session_id.clone(),
                                event_id: EventId::new(),
                                event: Event::Agent(AgentEvent::AskUserQuestion {
                                    req_id: req_id.clone(),
                                    session_id: session_id.0.to_string(),
                                    questions: questions.clone(),
                                }),
                            },
                        );
                    }
                    continue;
                }
                Event::Agent(AgentEvent::PermissionRequest {
                    req_id,
                    tool_id,
                    tool_name,
                    tool_args,
                    tool_level,
                    reason,
                    ..
                }) => {
                    if let Some(ref bus) = self.shared.event_bus {
                        let _ = bus.publish(
                            self.parent_session_id.clone(),
                            crate::event::Envelope {
                                session_id: self.parent_session_id.clone(),
                                event_id: EventId::new(),
                                event: Event::Agent(AgentEvent::PermissionRequest {
                                    req_id: req_id.clone(),
                                    session_id: session_id.0.to_string(),
                                    tool_id: tool_id.clone(),
                                    tool_name: tool_name.clone(),
                                    tool_args: tool_args.clone(),
                                    tool_level: tool_level.clone(),
                                    reason: reason.clone(),
                                }),
                            },
                        );
                    }
                    continue;
                }
                Event::Agent(AgentEvent::PermissionAck { req_id }) => {
                    if let Some(ref bus) = self.shared.event_bus {
                        let _ = bus.publish(
                            self.parent_session_id.clone(),
                            crate::event::Envelope {
                                session_id: self.parent_session_id.clone(),
                                event_id: EventId::new(),
                                event: Event::Agent(AgentEvent::PermissionAck {
                                    req_id: req_id.clone(),
                                }),
                            },
                        );
                    }
                    continue;
                }
                Event::Agent(AgentEvent::AskUserAck { req_id }) => {
                    if let Some(ref bus) = self.shared.event_bus {
                        let _ = bus.publish(
                            self.parent_session_id.clone(),
                            crate::event::Envelope {
                                session_id: self.parent_session_id.clone(),
                                event_id: EventId::new(),
                                event: Event::Agent(AgentEvent::AskUserAck {
                                    req_id: req_id.clone(),
                                }),
                            },
                        );
                    }
                    continue;
                }
                _ => {}
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
                    "description": "Instructions for the agent. Brief clearly - what to do, why, and expected output."
                },
                "wait_for_completion": {
                    "type": "boolean",
                    "description": "Whether to wait for the subagent to finish before returning. Default true (sync). Set to false to launch in background and receive the result as a follow-up message.",
                    "default": true
                },
                "agent_id": {
                    "type": "string",
                    "description": "Optional existing subagent session ID to reuse (e.g., 'sub_xxx'). Omit to create a new agent. Reusing preserves the agent's memory and state across calls."
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

        let wait = args["wait_for_completion"].as_bool().unwrap_or(true);
        let mode = if wait {
            SubAgentMode::Sync
        } else {
            SubAgentMode::Async
        };

        let agent_id = args["agent_id"].as_str().map(SessionId::from);

        // TODO(#subagent-resume): When agent_id is not provided and this call
        // is being re-executed after a crash (e.g. parent resumed with
        // pending_tool_calls), look up the history message_store by
        // ctx.tool_call_id to find a prior Role::Internal message whose
        // metadata contains "subagent_session_id", then reuse that id
        // instead of creating a new one.

        tracing::info!("spawning sub-agent (reuse: {})", agent_id.is_some());

        // Prevent recursive spawning
        if self.parent_session_id.starts_with(crate::types::SUB_PREFIX) {
            return Ok(ToolOutput::error(
                "Sub-agents cannot spawn other sub-agents. Use the parent agent to coordinate multiple tasks.",
            ));
        }

        // Validate agent_id if provided
        if let Some(ref sid) = agent_id {
            if !sid.starts_with(crate::types::SUB_PREFIX) {
                return Ok(ToolOutput::error(format!(
                    "agent_id '{}' is not a valid subagent session id (must start with '{}')",
                    sid,
                    crate::types::SUB_PREFIX
                )));
            }
            let exists = if let Some(ref store) = self.shared.session_store {
                store.get(sid).await.is_ok_and(|opt| opt.is_some())
            } else if let Some(ref store) = self.shared.message_store {
                store
                    .get(sid.as_str())
                    .await
                    .is_ok_and(|msgs| !msgs.is_empty())
            } else {
                false
            };
            if !exists {
                return Ok(ToolOutput::error(format!(
                    "agent_id '{sid}' does not refer to an existing subagent session"
                )));
            }
        }

        // Reuse existing session or create a new one
        let session_id = agent_id.clone().unwrap_or_else(SessionId::new_subagent);
        let is_reuse = agent_id.is_some();
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
            metadata.insert("parent_tool_id".to_string(), ctx.tool_call_id.to_string());
            if let Err(e) = bus.publish(
                self.parent_session_id.clone(),
                crate::event::Envelope {
                    session_id: self.parent_session_id.clone(),
                    event_id: EventId::new(),
                    event: crate::event::Event::Tool(crate::event::ToolEvent::Metadata {
                        message_id: ctx.message_id.clone(),
                        tool_id: ctx.tool_call_id.to_string(),
                        metadata,
                    }),
                },
            ) {
                tracing::warn!("Failed to publish subagent metadata: {}", e);
            }
        }

        // Persist subagent session to database only when creating a new session.
        // Store failures are non-fatal: warn and continue so the subagent can still run.
        if !is_reuse {
            if let Some(ref store) = self.shared.session_store {
                let parent = match store.get(&self.parent_session_id).await {
                    Ok(Some(info)) => Some(info),
                    Ok(None) => {
                        tracing::warn!(
                            "parent session {} not found; creating subagent session without \
                             inherited metadata",
                            self.parent_session_id.0
                        );
                        None
                    }
                    Err(e) => {
                        tracing::warn!("failed to get parent session metadata: {}", e);
                        None
                    }
                };
                let (project_id, working_dir, auto_approve_level, model_key) =
                    parent.map_or((None, None, None, None), |p| {
                        (
                            p.project_id,
                            p.working_dir,
                            p.auto_approve_level,
                            p.model_key,
                        )
                    });
                if let Err(e) = store
                    .create(
                        &session_id,
                        project_id.as_ref(),
                        working_dir.as_deref(),
                        auto_approve_level.as_deref(),
                        Some(&self.parent_session_id),
                        model_key.as_deref(),
                    )
                    .await
                {
                    tracing::warn!("failed to create subagent session record: {}", e);
                }
            }
        }
        if let Some(ref store) = self.shared.session_store {
            if let Err(e) = store.update_title(&session_id, &description).await {
                tracing::warn!("failed to set subagent session title: {}", e);
            }
        }

        let params = RunSubagentParams {
            session_id: session_id.clone(),
            prompt: prompt_clone,
        };

        let result = match mode {
            SubAgentMode::Async => {
                let result_text = agent_prefix(
                    &session_id,
                    format!(
                        "Subagent with task '{description}' spawned in async mode. {} Results will be sent automatically when complete.",
                        crate::tools::ASYNC_LAUNCH_GUIDE
                    ),
                );
                let cancel = ctx
                    .cancel_token
                    .expect("cancel_token must be available for subagent tool");
                let self_clone = self.clone();
                tokio::spawn(async move {
                    let (output, status) = tokio::select! {
                        biased;
                        () = cancel.cancelled() => {
                            (String::new(), SubAgentStatus::Cancelled)
                        }
                        result = self_clone.run_subagent(params) => result,
                    };
                    let steer = match &status {
                        SubAgentStatus::Completed => agent_prefix(
                            &session_id,
                            format!("Task '{description}' completed:\n\n{output}"),
                        ),
                        SubAgentStatus::Failed(e) => agent_prefix(
                            &session_id,
                            format!("Task '{description}' failed:\n\n{output}\n\n[error: {e}]"),
                        ),
                        SubAgentStatus::Cancelled => agent_prefix(
                            &session_id,
                            format!("Task '{description}' cancelled:\n\n{output}\n\n[cancelled]"),
                        ),
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
                    SubAgentStatus::Completed => {
                        ToolOutput::text(agent_prefix(&session_id, &output))
                    }
                    SubAgentStatus::Failed(e) => {
                        if output.is_empty() {
                            ToolOutput::error(e.clone())
                        } else {
                            ToolOutput::text(agent_prefix(
                                &session_id,
                                format!("{output}\n\n[error: {e}]"),
                            ))
                        }
                    }
                    SubAgentStatus::Cancelled => {
                        if output.is_empty() {
                            ToolOutput::text(agent_prefix(&session_id, "cancelled"))
                        } else {
                            ToolOutput::text(agent_prefix(
                                &session_id,
                                format!("{output}\n\n[cancelled]"),
                            ))
                        }
                    }
                }
            }
        };

        Ok(result)
    }
}
