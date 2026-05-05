//! Kernel event processing

use tuirealm::props::{AttrValue, Attribute};

use kernel::event::{AgentStatus, Event, StopReason, SystemEvent};
use kernel::tools::TODO_WRITE_TOOL_NAME;
use kernel::types::FinishReason;

use crate::app::notifications::send_desktop_notification;
use crate::app::state::StreamingStatus;
use crate::app::streaming::StreamingOps;
use crate::app::types::Model;
use crate::app::ui_ops::UiOps;
use crate::attr;
use crate::components::info_bar::Notification;
use crate::id::Id;

/// Event processing trait
pub trait EventHandler {
    fn process_kernel_events(&mut self) -> anyhow::Result<()>;
}

impl EventHandler for Model {
    fn process_kernel_events(&mut self) -> anyhow::Result<()> {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                Event::User(kernel::event::UserEvent::Message { content }) => {
                    self.handle_user_message(content);
                }
                Event::Model(kernel::event::ModelEvent::Chunk { content, .. }) => {
                    self.handle_model_chunk(content);
                }
                Event::Model(kernel::event::ModelEvent::ToolCallDelta {
                    tool_name,
                    arguments_delta,
                    ..
                }) => {
                    self.handle_tool_call_delta(&tool_name, &arguments_delta);
                }
                Event::Model(kernel::event::ModelEvent::Error { error, .. }) => {
                    self.handle_model_error(&error);
                }
                Event::Model(kernel::event::ModelEvent::Request { .. }) => {
                    self.start_streaming();
                }
                Event::Model(kernel::event::ModelEvent::Compacting { active, .. }) => {
                    self.handle_compacting(active);
                }
                Event::Model(kernel::event::ModelEvent::TokenUsage {
                    total_tokens,
                    context_window,
                    ..
                }) => {
                    self.handle_token_usage(total_tokens, context_window);
                }
                Event::Tool(kernel::event::ToolEvent::Started {
                    tool_id,
                    tool_name,
                    arguments,
                    ..
                }) => {
                    self.handle_tool_started(&tool_id, &tool_name, arguments.as_deref());
                }
                Event::Tool(kernel::event::ToolEvent::Output {
                    tool_id,
                    output,
                    content_blocks,
                    elapsed_ms,
                    ..
                }) => {
                    self.handle_tool_output(&tool_id, &output, content_blocks, elapsed_ms);
                }
                Event::Tool(kernel::event::ToolEvent::Error {
                    tool_id,
                    error,
                    elapsed_ms,
                    ..
                }) => {
                    self.handle_tool_error(&tool_id, &error, elapsed_ms);
                }
                Event::Tool(kernel::event::ToolEvent::Progress {
                    tool_id,
                    message,
                    tokens,
                    ..
                }) => {
                    self.handle_tool_progress(&tool_id, &message, tokens);
                }
                Event::Agent(kernel::event::AgentEvent::Lifecycle { state, .. }) => {
                    self.handle_agent_lifecycle(state);
                }
                Event::Agent(kernel::event::AgentEvent::Error {
                    phase,
                    error,
                    is_recoverable,
                    ..
                }) => {
                    self.handle_agent_error(phase, &error, is_recoverable);
                }
                Event::Agent(kernel::event::AgentEvent::Retrying {
                    attempt,
                    max_attempts,
                    reason,
                    ..
                }) => {
                    self.handle_retrying(attempt, max_attempts, &reason);
                }
                Event::System(SystemEvent::Shutdown {
                    error: Some(err), ..
                }) => {
                    self.handle_shutdown_error(&err);
                }
                Event::Agent(kernel::event::AgentEvent::PermissionRequest {
                    req_id,
                    tool_name,
                    tool_args,
                    tool_level,
                    ..
                }) => {
                    self.handle_permission_request(&req_id, &tool_name, &tool_args, tool_level);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Individual event handlers
pub trait EventHandlers {
    fn handle_user_message(&mut self, content: Vec<kernel::types::ContentBlock>);
    fn handle_model_chunk(&mut self, content: kernel::event::ContentChunk);
    fn handle_tool_call_delta(&mut self, tool_name: &str, arguments_delta: &str);
    fn handle_model_error(&mut self, error: &str);
    fn handle_compacting(&mut self, active: bool);
    fn handle_token_usage(&mut self, total_tokens: u32, context_window: u32);
    fn handle_tool_started(&mut self, tool_id: &str, tool_name: &str, arguments: Option<&str>);
    fn handle_tool_output(
        &mut self,
        tool_id: &str,
        output: &str,
        content_blocks: Vec<kernel::types::ToolOutputBlock>,
        elapsed_ms: u64,
    );
    fn handle_tool_error(&mut self, tool_id: &str, error: &str, elapsed_ms: u64);
    fn handle_tool_progress(&mut self, tool_id: &str, message: &str, tokens: Option<u32>);
    fn handle_agent_lifecycle(&mut self, state: AgentStatus);
    fn handle_agent_error(&mut self, phase: kernel::event::ErrorPhase, error: &str, is_recoverable: bool);
    fn handle_retrying(&mut self, attempt: u32, max_attempts: u32, reason: &str);
    fn handle_shutdown_error(&mut self, err: &str);
    fn handle_permission_request(
        &mut self,
        req_id: &str,
        tool_name: &str,
        tool_args: &str,
        tool_level: String,
    );
}

impl EventHandlers for Model {
    fn handle_user_message(&mut self, content: Vec<kernel::types::ContentBlock>) {
        let blocks_json = serde_json::to_string(&content).unwrap_or_default();
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::ADD_USER_MESSAGE),
            AttrValue::String(blocks_json),
        );
        self.scroll_chat_to_bottom();
        self.state.should_redraw = true;
    }

    fn handle_model_chunk(&mut self, content: kernel::event::ContentChunk) {
        self.state.is_streaming = true;
        // Clear tool call delta when receiving regular content
        self.clear_tool_call_delta();
        match content {
            kernel::event::ContentChunk::Text(text) => {
                self.append_streaming_content(&text, false);
            }
            kernel::event::ContentChunk::Thinking { thinking, .. } => {
                self.append_streaming_content(&thinking, true);
            }
            kernel::event::ContentChunk::RedactedThinking => {}
        }
    }

    fn handle_tool_call_delta(&mut self, tool_name: &str, arguments_delta: &str) {
        let attr = Attribute::Custom(attr::APPEND_TOOL_CALL_DELTA);
        let value = AttrValue::String(format!("{tool_name}\x00{arguments_delta}"));
        let _ = self.app.attr(&Id::InfoBar, attr, value);
        self.state.should_redraw = true;
    }

    fn handle_model_error(&mut self, error: &str) {
        self.handle_streaming_error(
            StreamingStatus::Failed,
            format!("Model error: {error}"),
        );
    }

    fn handle_compacting(&mut self, active: bool) {
        let attr = if active {
            Attribute::Custom(attr::START_COMPACTING)
        } else {
            Attribute::Custom(attr::STOP_COMPACTING)
        };
        let _ = self.app.attr(&Id::InfoBar, attr, AttrValue::Flag(active));
        self.state.should_redraw = true;
    }

    fn handle_token_usage(&mut self, total_tokens: u32, context_window: u32) {
        let usage_str = format!("{total_tokens}\x00{context_window}");
        let _ = self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_CTX_USAGE),
            AttrValue::String(usage_str),
        );
        self.state.should_redraw = true;
    }

    fn handle_tool_started(&mut self, tool_id: &str, tool_name: &str, arguments: Option<&str>) {
        let args_str = arguments.unwrap_or_default();
        let combined = format!("{tool_id}\x00{tool_name}\x00{args_str}");
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::START_TOOL),
            AttrValue::String(combined),
        );

        // Handle todoWrite tool - update todo list panel
        if tool_name == TODO_WRITE_TOOL_NAME {
            if let Some(args) = arguments {
                let _ = self.app.attr(
                    &Id::TodoList,
                    Attribute::Custom(attr::SET_TODOS),
                    AttrValue::String(args.to_string()),
                );
            }
        }

        self.state.should_redraw = true;
    }

    fn handle_tool_output(
        &mut self,
        tool_id: &str,
        output: &str,
        content_blocks: Vec<kernel::types::ToolOutputBlock>,
        elapsed_ms: u64,
    ) {
        self.clear_tool_call_delta();
        let blocks_json = serde_json::to_string(&content_blocks).unwrap_or_default();
        let combined = format!("{tool_id}\x00{output}\x00{elapsed_ms}\x00{blocks_json}");
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::COMPLETE_TOOL),
            AttrValue::String(combined),
        );
        self.state.should_redraw = true;

        // Windows workaround: re-enable mouse capture after shell commands
        #[cfg(target_os = "windows")]
        {
            let _ = self.terminal.enable_mouse_capture();
        }
    }

    fn handle_tool_error(&mut self, tool_id: &str, error: &str, elapsed_ms: u64) {
        self.clear_tool_call_delta();
        let combined = format!("{tool_id}\x00{error}\x00{elapsed_ms}");
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::FAIL_TOOL),
            AttrValue::String(combined),
        );
        self.state.should_redraw = true;

        // Windows workaround: re-enable mouse capture after shell commands
        #[cfg(target_os = "windows")]
        {
            let _ = self.terminal.enable_mouse_capture();
        }
    }

    fn handle_tool_progress(&mut self, tool_id: &str, message: &str, tokens: Option<u32>) {
        let tokens_str = tokens.map(|t| t.to_string()).unwrap_or_default();
        let combined = format!("{tool_id}\x00{message}\x00{tokens_str}");
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::UPDATE_TOOL_PROGRESS),
            AttrValue::String(combined),
        );
        self.state.should_redraw = true;
    }

    fn handle_agent_lifecycle(&mut self, state: AgentStatus) {
        match state {
            AgentStatus::Running => {
                // Agent started - could show in status bar if needed
            }
            AgentStatus::IterationCompleted {
                iteration,
                messages,
            } => {
                tracing::debug!("Iteration {iteration} completed with {messages} messages");
            }
            AgentStatus::TurnCompleted {
                total_iterations,
                finish_reason,
            } => {
                self.handle_turn_completed(total_iterations as u32, finish_reason);
            }
            AgentStatus::Stopped { reason } => {
                self.handle_agent_stopped(reason);
            }
        }
    }

    fn handle_agent_error(&mut self, phase: kernel::event::ErrorPhase, error: &str, is_recoverable: bool) {
        let phase_str = format!("{phase:?}");
        if is_recoverable {
            let message = format!(" {phase_str} error (will retry): {error}");
            self.show_notification(&Notification::warn(message, 3000));
            self.state.should_redraw = true;
        } else {
            self.handle_streaming_error(
                StreamingStatus::Failed,
                format!("{phase_str} error: {error}"),
            );
        }
    }

    fn handle_retrying(&mut self, attempt: u32, max_attempts: u32, reason: &str) {
        let message = format!(" Retrying ({attempt}/{max_attempts}): {reason}");
        self.show_notification(&Notification::info(message, 0));
        self.state.should_redraw = true;
    }

    fn handle_shutdown_error(&mut self, err: &str) {
        self.finalize_assistant_message();
        self.handle_streaming_error(
            StreamingStatus::Failed,
            format!("Session closed with error: {err}"),
        );
        self.state.should_redraw = true;
    }

    fn handle_permission_request(
        &mut self,
        req_id: &str,
        tool_name: &str,
        tool_args: &str,
        tool_level: String,
    ) {
        tracing::info!("TUI received PermissionRequest: {} for {}", req_id, tool_name);
        self.pending_permission = Some(req_id.to_string());

        let message = format!("Tool: {tool_name}\nLevel: {tool_level}\nArgs: {tool_args}");
        let dialog_data = format!(
            "Can I run this tool?\x00Sure\x00Always allow this tool with level {tool_level}\x00Not now\x00YOLO - allow all dangerous tools\x00{message}"
        );
        tracing::debug!("Showing dialog with data: {dialog_data}");
        let _ = self.app.attr(
            &Id::Dialog,
            Attribute::Custom(attr::DIALOG_SHOW),
            AttrValue::String(dialog_data),
        );
        let result = self.app.active(&Id::Dialog);
        tracing::debug!("Dialog focus result: {:?}", result);
        self.state.should_redraw = true;
    }
}

/// Helper methods for agent lifecycle
trait AgentLifecycleHelper {
    fn handle_turn_completed(&mut self, total_iterations: u32, finish_reason: Option<FinishReason>);
    fn handle_agent_stopped(&mut self, reason: StopReason);
}

impl AgentLifecycleHelper for Model {
    fn handle_turn_completed(&mut self, total_iterations: u32, finish_reason: Option<FinishReason>) {
        match finish_reason {
            Some(FinishReason::MaxTokens) => {
                let message = " Response truncated: max tokens reached";
                send_desktop_notification("Yomi - Stopped", message);
                self.handle_streaming_error(StreamingStatus::Failed, message.to_string());
            }
            Some(FinishReason::ContentFilter) => {
                let message = " Response blocked: content filter triggered";
                send_desktop_notification("Yomi - Stopped", message);
                self.handle_streaming_error(StreamingStatus::Failed, message.to_string());
            }
            _ => {
                self.finalize_assistant_message();
                self.stop_streaming(StreamingStatus::Completed);
                let message = format!("😸 Task completed ({total_iterations} iterations)");
                send_desktop_notification("Yomi", &message);
                self.show_notification(&Notification::success(&message, 5000));
            }
        }
        self.state.should_redraw = true;
    }

    fn handle_agent_stopped(&mut self, reason: StopReason) {
        match reason {
            StopReason::Completed => {
                // Normal completion - already handled by TurnCompleted
            }
            StopReason::Cancelled { operation } => {
                let message = operation.map_or_else(
                    || " Cancelled".to_string(),
                    |op| format!(" Cancelled: {op}"),
                );
                self.handle_streaming_error(StreamingStatus::Cancelled, message);
            }
            StopReason::Failed { error } => {
                self.finalize_assistant_message();
                let message = format!(" Task failed: {error}");
                send_desktop_notification("Yomi - Error", &message);
                self.handle_streaming_error(
                    StreamingStatus::Failed,
                    format!("Agent error: {error}"),
                );
            }
            StopReason::MaxIterations { reached } => {
                self.finalize_assistant_message();
                let message = format!(" Max iterations reached ({reached})");
                send_desktop_notification("Yomi - Stopped", &message);
                self.handle_streaming_error(
                    StreamingStatus::MaxIterations,
                    format!("Reached maximum iterations ({reached})"),
                );
            }
        }
    }
}
