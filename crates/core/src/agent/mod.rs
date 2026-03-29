mod config;
mod state;

pub use config::{AgentConfig, SubAgentMode};
pub use state::AgentState;

use crate::bus::EventBus;
use crate::event::{AgentEvent, AgentResult, ModelEvent, ToolEvent, ContentChunk};
use crate::provider::{ModelProvider, ModelStreamItem};
use crate::storage::Storage;
use crate::tool::{ToolRegistry, ToolSandbox};
use anyhow::Result;
use futures::TryStreamExt;
use nekoclaw_shared::types::{AgentId, ContentBlock, Message, Role, ToolCall};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Agent {
    id: AgentId,
    state: AgentState,
    config: AgentConfig,
    messages: Vec<Message>,
    event_bus: EventBus,
    provider: Arc<dyn ModelProvider>,
    storage: Arc<dyn Storage>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    input_rx: mpsc::Receiver<String>,
    sub_agents: Vec<AgentHandle>,
    iteration_count: usize,
}

pub struct AgentHandle {
    pub id: AgentId,
    pub mode: SubAgentMode,
}

impl Agent {
    pub fn new(
        id: AgentId,
        config: AgentConfig,
        event_bus: EventBus,
        provider: Arc<dyn ModelProvider>,
        storage: Arc<dyn Storage>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
        input_rx: mpsc::Receiver<String>,
    ) -> Self {
        let mut messages = Vec::new();
        messages.push(Message::system(&config.system_prompt));
        Self {
            id,
            state: AgentState::Idle,
            config,
            messages,
            event_bus,
            provider,
            storage,
            tool_registry,
            sandbox,
            input_rx,
            sub_agents: Vec::new(),
            iteration_count: 0,
        }
    }

    pub fn id(&self) -> &AgentId {
        &self.id
    }

    pub fn state(&self) -> AgentState {
        self.state
    }

    pub fn spawn(mut self) -> AgentId {
        let id = self.id.clone();
        let id_str = id.0.clone();
        tokio::spawn(async move {
            if let Err(e) = self.run().await {
                tracing::error!("Agent {} failed: {}", id_str, e);
                self.transition_to(AgentState::Failed);
            }
        });
        id
    }

    async fn run(&mut self) -> Result<()> {
        self.transition_to(AgentState::WaitingForInput);
        loop {
            if self.state.is_terminal() {
                break;
            }
            if self.iteration_count >= self.config.max_iterations {
                tracing::warn!("Max iterations reached, forcing completion");
                self.transition_to(AgentState::Completed);
                break;
            }
            match self.state {
                AgentState::WaitingForInput => self.handle_wait_for_input().await?,
                AgentState::Streaming => self.handle_streaming().await?,
                AgentState::ExecutingTool => self.handle_execute_tool().await?,
                _ => tokio::task::yield_now().await,
            }
            self.iteration_count += 1;
        }
        let result = AgentResult {
            messages: self.messages.clone(),
            tool_calls: self.count_tool_calls(),
        };
        self.event_bus.send(crate::event::Event::Agent(
            AgentEvent::Completed {
                agent_id: self.id.clone(),
                result,
            },
        ))?;
        Ok(())
    }

    async fn handle_wait_for_input(&mut self) -> Result<()> {
        match self.input_rx.recv().await {
            Some(content) => {
                self.messages.push(Message::user(content));
                self.transition_to(AgentState::Streaming);
                Ok(())
            }
            None => {
                self.transition_to(AgentState::Cancelled);
                Ok(())
            }
        }
    }

    async fn handle_streaming(&mut self) -> Result<()> {
        let tools = self.tool_registry.definitions();
        self.event_bus.send(crate::event::Event::Model(ModelEvent::Request {
            agent_id: self.id.clone(),
            message_count: self.messages.len(),
        }))?;
        let mut stream = self.provider.stream(&self.messages, &tools, &self.config.model).await?;
        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut current_text = String::new();
        let mut current_thinking = String::new();
        let mut pending_tool_calls: Vec<ToolCall> = Vec::new();
        while let Some(item) = stream.try_next().await? {
            match item {
                ModelStreamItem::Chunk(ContentChunk::Text(text)) => {
                    current_text.push_str(&text);
                    self.event_bus.send(crate::event::Event::Model(ModelEvent::Chunk {
                        agent_id: self.id.clone(),
                        content: ContentChunk::Text(text),
                    }))?;
                }
                ModelStreamItem::Chunk(ContentChunk::Thinking { thinking, signature }) => {
                    current_thinking.push_str(&thinking);
                    self.event_bus.send(crate::event::Event::Model(ModelEvent::Chunk {
                        agent_id: self.id.clone(),
                        content: ContentChunk::Thinking { thinking, signature },
                    }))?;
                }
                ModelStreamItem::Chunk(ContentChunk::RedactedThinking) => {
                    content_blocks.push(ContentBlock::RedactedThinking { data: String::new() });
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
                    self.event_bus.send(crate::event::Event::Model(ModelEvent::Fallback {
                        agent_id: self.id.clone(),
                        from,
                        to,
                    }))?;
                }
            }
            if current_text.len() + current_thinking.len() % 1000 == 0 {
                tokio::task::yield_now().await;
            }
        }
        if !current_thinking.is_empty() {
            content_blocks.push(ContentBlock::Thinking { thinking: current_thinking, signature: None });
        }
        if !current_text.is_empty() {
            content_blocks.push(ContentBlock::Text { text: current_text });
        }
        if !content_blocks.is_empty() || !pending_tool_calls.is_empty() {
            let mut msg = Message::with_blocks(Role::Assistant, content_blocks);
            if !pending_tool_calls.is_empty() {
                msg.tool_calls = Some(pending_tool_calls);
            }
            self.messages.push(msg);
        }
        if self.messages.last().and_then(|m| m.tool_calls.as_ref()).is_some() {
            self.transition_to(AgentState::ExecutingTool);
        } else {
            self.transition_to(AgentState::WaitingForInput);
        }
        Ok(())
    }

    async fn handle_execute_tool(&mut self) -> Result<()> {
        let tool_calls = self.messages.last().and_then(|m| m.tool_calls.clone()).unwrap_or_default();
        for call in tool_calls {
            self.event_bus.send(crate::event::Event::Tool(ToolEvent::Started {
                agent_id: self.id.clone(),
                tool_id: call.id.clone(),
                tool_name: call.name.clone(),
            }))?;
            let tool = match self.tool_registry.get(&call.name) {
                Some(t) => t,
                None => {
                    self.event_bus.send(crate::event::Event::Tool(ToolEvent::Error {
                        agent_id: self.id.clone(),
                        tool_id: call.id.clone(),
                        error: format!("Unknown tool: {}", call.name),
                    }))?;
                    continue;
                }
            };
            if let Err(e) = tool.is_allowed(&call.arguments).await {
                self.event_bus.send(crate::event::Event::Tool(ToolEvent::Error {
                    agent_id: self.id.clone(),
                    tool_id: call.id.clone(),
                    error: e.to_string(),
                }))?;
                continue;
            }
            match tool.execute(call.arguments).await {
                Ok(output) => {
                    let content = if output.success() {
                        output.stdout.clone()
                    } else {
                        format!("Exit code: {}\n{}\n{}", output.exit_code, output.stdout, output.stderr)
                    };
                    self.event_bus.send(crate::event::Event::Tool(ToolEvent::Output {
                        agent_id: self.id.clone(),
                        tool_id: call.id.clone(),
                        output: content.clone(),
                    }))?;
                    self.messages.push(Message {
                        role: Role::Tool,
                        content: vec![ContentBlock::Text { text: content }],
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        created_at: chrono::Utc::now(),
                    });
                }
                Err(e) => {
                    self.event_bus.send(crate::event::Event::Tool(ToolEvent::Error {
                        agent_id: self.id.clone(),
                        tool_id: call.id.clone(),
                        error: e.to_string(),
                    }))?;
                }
            }
            tokio::task::yield_now().await;
        }
        self.transition_to(AgentState::Streaming);
        Ok(())
    }

    fn transition_to(&mut self, new_state: AgentState) {
        let old_state = self.state;
        self.state = new_state;
        if old_state != new_state {
            self.event_bus.send(crate::event::Event::Agent(AgentEvent::StateChanged {
                agent_id: self.id.clone(),
                state: new_state.to_string(),
            })).ok();
        }
    }

    fn count_tool_calls(&self) -> usize {
        self.messages.iter().filter_map(|m| m.tool_calls.as_ref().map(|c| c.len())).sum()
    }

    pub fn spawn_sub_agent(&mut self, mode: SubAgentMode) -> AgentId {
        let child_id = AgentId::new();
        self.sub_agents.push(AgentHandle {
            id: child_id.clone(),
            mode,
        });
        self.event_bus.send(crate::event::Event::Agent(AgentEvent::SubAgentSpawned {
            parent_id: self.id.clone(),
            child_id: child_id.clone(),
            mode: mode.to_string(),
        })).ok();
        child_id
    }
}
