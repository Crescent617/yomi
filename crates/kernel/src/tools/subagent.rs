use crate::agent::{Agent, AgentInput, AgentShared, SubAgentMode};
use crate::event::Event;
use crate::tool::Tool;
use crate::types::{AgentId, ContentBlock, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Tool for spawning sub-agents to handle specific tasks
pub struct SubAgentTool {
    parent_id: AgentId,
    shared: Arc<AgentShared>,
    /// Parent's `input_tx` for forwarding async sub-agent results
    parent_input_tx: mpsc::Sender<AgentInput>,
}

impl SubAgentTool {
    pub const fn new(
        parent_id: AgentId,
        shared: Arc<AgentShared>,
        parent_input_tx: mpsc::Sender<AgentInput>,
    ) -> Self {
        Self {
            parent_id,
            shared,
            parent_input_tx,
        }
    }
}

#[async_trait]
impl Tool for SubAgentTool {
    fn name(&self) -> &'static str {
        "spawn_subagent"
    }

    fn desc(&self) -> &'static str {
        "Spawn a sub-agent to handle a specific task. \
         Use 'sync' mode to wait for completion and get results, \
         or 'async' mode to spawn and continue immediately."
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The specific task description for the sub-agent"
                },
                "mode": {
                    "type": "string",
                    "enum": ["async", "sync"],
                    "description": "Execution mode: 'async' returns immediately with sub-agent ID, 'sync' waits for completion and returns results",
                    "default": "sync"
                }
            },
            "required": ["task"]
        })
    }

    async fn exec(&self, args: Value) -> Result<ToolOutput> {
        let task = args["task"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'task' argument"))?;

        let mode_str = args["mode"].as_str().unwrap_or("sync");
        let mode = match mode_str {
            "async" => SubAgentMode::Async,
            _ => SubAgentMode::Sync,
        };

        tracing::info!(
            "Spawning sub-agent for parent {} with task: {}",
            self.parent_id,
            task
        );

        // Spawn the sub-agent
        let (handle, mut event_rx) = Agent::spawn(
            AgentId::new(),
            &self.shared,
            &format!(
                "You are a sub-agent. Parent: {}. Task: {}",
                self.parent_id, task
            ),
            None,
            None,
            10,
            false,
            SubAgentMode::Async,
        );

        let sub_agent_id = handle.id.clone();

        // Send the task (as text content block)
        handle.send_text(task.to_string()).await.ok();

        match mode {
            SubAgentMode::Async => {
                // Spawn background task to collect results and forward to parent
                let parent_tx = self.parent_input_tx.clone();
                let sub_id = sub_agent_id.clone();
                tokio::spawn(async move {
                    let mut output = String::new();
                    let mut completed = false;

                    while let Some(event) = event_rx.recv().await {
                        match event {
                            Event::Model(crate::event::ModelEvent::Chunk { content, .. }) => {
                                use crate::event::ContentChunk;
                                match content {
                                    ContentChunk::Text(text) => output.push_str(&text),
                                    ContentChunk::Thinking { thinking, .. } => {
                                        output.push_str("\n[Thinking]: ");
                                        output.push_str(&thinking);
                                    }
                                    ContentChunk::RedactedThinking => {
                                        output.push_str("\n[Redacted thinking]");
                                    }
                                }
                            }
                            Event::Agent(crate::event::AgentEvent::Completed { .. }) => {
                                completed = true;
                                break;
                            }
                            Event::Agent(crate::event::AgentEvent::Failed { error, .. }) => {
                                use std::fmt::Write;
                                let _ = write!(output, "\n[Sub-agent failed: {error}]");
                                break;
                            }
                            Event::Agent(crate::event::AgentEvent::Cancelled { .. }) => {
                                output.push_str("\n[Sub-agent was cancelled]");
                                break;
                            }
                            _ => {}
                        }
                    }

                    // Forward result to parent agent
                    let result_text = if completed {
                        format!("\n\n[Async Sub-agent {sub_id} completed]\nResult:\n{output}")
                    } else {
                        format!("\n\n[Async Sub-agent {sub_id} ended]\nPartial result:\n{output}")
                    };

                    // Send result back to parent via input_tx (as ContentBlock array)
                    let _ = parent_tx
                        .send(AgentInput::ToolResult {
                            tool_id: format!("subagent_{sub_id}"),
                            content: vec![ContentBlock::Text { text: result_text }],
                        })
                        .await;
                });

                let result = format!(
                    "Sub-agent {sub_agent_id} spawned in async mode. Results will be sent when complete. Task: {task}"
                );
                Ok(ToolOutput {
                    stdout: result,
                    stderr: String::new(),
                    exit_code: 0,
                })
            }
            SubAgentMode::Sync => {
                // Collect all output from sub-agent
                let mut output = String::new();
                let mut completed = false;

                while let Some(event) = event_rx.recv().await {
                    match event {
                        Event::Model(crate::event::ModelEvent::Chunk { content, .. }) => {
                            use crate::event::ContentChunk;
                            match content {
                                ContentChunk::Text(text) => output.push_str(&text),
                                ContentChunk::Thinking { thinking, .. } => {
                                    output.push_str("\n[Thinking]: ");
                                    output.push_str(&thinking);
                                }
                                ContentChunk::RedactedThinking => {
                                    output.push_str("\n[Redacted thinking]");
                                }
                            }
                        }
                        Event::Agent(crate::event::AgentEvent::Completed { .. }) => {
                            completed = true;
                            break;
                        }
                        Event::Agent(crate::event::AgentEvent::Failed { error, .. }) => {
                            return Ok(ToolOutput {
                                stdout: output,
                                stderr: format!("Sub-agent failed: {error}"),
                                exit_code: 1,
                            });
                        }
                        Event::Agent(crate::event::AgentEvent::Cancelled { .. }) => {
                            return Ok(ToolOutput {
                                stdout: output,
                                stderr: "Sub-agent was cancelled".to_string(),
                                exit_code: 1,
                            });
                        }
                        _ => {}
                    }
                }

                if completed {
                    use std::fmt::Write;
                    let _ = write!(output, "\n\n[Sub-agent {sub_agent_id} completed]");
                    Ok(ToolOutput {
                        stdout: output,
                        stderr: String::new(),
                        exit_code: 0,
                    })
                } else {
                    Ok(ToolOutput {
                        stdout: output,
                        stderr: "Sub-agent ended without completion".to_string(),
                        exit_code: 1,
                    })
                }
            }
        }
    }
}
