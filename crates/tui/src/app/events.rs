//! Kernel event processing

use anyhow::Result;
use tuirealm::props::{AttrValue, Attribute};
#[cfg(windows)]
use tuirealm::terminal::TerminalAdapter;

use crate::{attr, id::Id};
use kernel::event::{AgentStatus, Event, StopReason};
use kernel::tools::TODO_TOOL_NAME;
use kernel::types::FinishReason;

use super::types::{Model, StreamingStatus};

impl Model {
    /// Process events from kernel
    pub async fn process_kernel_event(&mut self) -> Result<()> {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                // User message from kernel (render after kernel accepts it)
                Event::User(kernel::event::UserEvent::Message { content, .. }) => {
                    let blocks_json = serde_json::to_string(&content).unwrap_or_default();
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::ADD_USER_MESSAGE),
                        AttrValue::String(blocks_json),
                    );
                    self.scroll_chat_to_bottom();
                    self.state.should_redraw = true;
                }
                Event::Model(kernel::event::ModelEvent::Chunk { content, .. }) => {
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
                Event::Model(kernel::event::ModelEvent::ToolCallDelta {
                    tool_name,
                    arguments_delta,
                    ..
                }) => {
                    // Update status bar to show tool call in progress
                    let attr = Attribute::Custom(attr::APPEND_TOOL_CALL_DELTA);
                    let value = AttrValue::String(format!("{tool_name}\x00{arguments_delta}"));
                    self.app.attr(&Id::InfoBar, attr, value)?;
                    self.state.should_redraw = true;
                }
                Event::Model(kernel::event::ModelEvent::Error { error, .. }) => {
                    // Model-level error: stop streaming and show error
                    self.handle_streaming_error(
                        StreamingStatus::Failed,
                        format!("Model error: {error}"),
                    );
                }
                Event::Model(kernel::event::ModelEvent::Request { .. }) => {
                    self.start_streaming();
                }
                Event::Model(kernel::event::ModelEvent::Compacting { active, .. }) => {
                    // Show/hide compacting status in InfoBar
                    let attr = if active {
                        Attribute::Custom(attr::START_COMPACTING)
                    } else {
                        Attribute::Custom(attr::STOP_COMPACTING)
                    };
                    self.app.attr(&Id::InfoBar, attr, AttrValue::Flag(active))?;
                    self.state.should_redraw = true;
                }
                Event::Model(kernel::event::ModelEvent::TokenUsage {
                    total_tokens,
                    context_window,
                    ..
                }) => {
                    // Update context window usage in status bar
                    let usage_str = format!("{total_tokens}\x00{context_window}");
                    self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom(attr::SET_CTX_USAGE),
                        AttrValue::String(usage_str),
                    )?;
                    self.state.should_redraw = true;
                }
                Event::Tool(kernel::event::ToolEvent::Start {
                    tool_id,
                    tool_name,
                    arguments,
                    ..
                }) => {
                    // Show tool execution start in chat view
                    let args_str = arguments.clone().unwrap_or_default();
                    let combined = format!("{tool_id}\x00{tool_name}\x00{args_str}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::START_TOOL),
                        AttrValue::String(combined),
                    )?;
                    self.state.should_redraw = true;
                }
                Event::Tool(kernel::event::ToolEvent::End {
                    tool_id,
                    content_blocks,
                    elapsed_ms,
                    tool_name,
                    is_error,
                    ..
                }) => {
                    // Clear tool call state from info bar (tool execution is complete)
                    self.clear_tool_call_delta();

                    // Extract text from content blocks
                    let output: String = content_blocks
                        .iter()
                        .filter_map(|block| match block {
                            kernel::types::ToolOutputBlock::Text { text } => Some(text.as_str()),
                            kernel::types::ToolOutputBlock::Image { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .concat();

                    if is_error {
                        // Show tool error in chat view
                        let combined = format!("{tool_id}\x00{output}\x00{elapsed_ms}");
                        self.app.attr(
                            &Id::ChatView,
                            Attribute::Custom(attr::FAIL_TOOL),
                            AttrValue::String(combined),
                        )?;
                    } else {
                        // Show tool output in chat view
                        // Format: tool_id\x00output\x00elapsed_ms\x00content_blocks_json
                        let blocks_json =
                            serde_json::to_string(&content_blocks).unwrap_or_default();
                        let combined =
                            format!("{tool_id}\x00{output}\x00{elapsed_ms}\x00{blocks_json}");
                        self.app.attr(
                            &Id::ChatView,
                            Attribute::Custom(attr::COMPLETE_TOOL),
                            AttrValue::String(combined),
                        )?;

                        if tool_name == TODO_TOOL_NAME {
                            // If the tool is a todo tool, refresh the todo list after completion
                            if let Err(e) = self.init_todo_list().await {
                                tracing::error!(
                                    "Failed to refresh todo list after tool execution: {}",
                                    e
                                );
                            }
                        }
                    }

                    self.state.should_redraw = true;

                    // Windows workaround: re-enable mouse capture after shell commands
                    // Shell tools may disable ENABLE_MOUSE_INPUT console mode on Windows
                    #[cfg(target_os = "windows")]
                    {
                        let _ = self.terminal.enable_mouse_capture();
                    }
                }
                Event::Tool(kernel::event::ToolEvent::Progress {
                    tool_id,
                    message,
                    tokens,
                    ..
                }) => {
                    // Update tool progress in chat view
                    // Format: tool_id\x00message\x00tokens (tokens is optional)
                    let tokens_str = tokens.map(|t| t.to_string()).unwrap_or_default();
                    let combined = format!("{tool_id}\x00{message}\x00{tokens_str}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::UPDATE_TOOL_PROGRESS),
                        AttrValue::String(combined),
                    )?;
                    self.state.should_redraw = true;
                }
                // Agent lifecycle state changes
                Event::Agent(kernel::event::AgentEvent::Lifecycle { state, .. }) => {
                    match state {
                        AgentStatus::Running => {
                            // Agent started - could show in status bar if needed
                        }
                        AgentStatus::TurnCompleted {
                            total_iterations,
                            finish_reason,
                            ..
                        } => {
                            // Task naturally completed - check for special finish reasons
                            match finish_reason {
                                Some(FinishReason::MaxTokens) => {
                                    let message = " Response truncated: max tokens reached";
                                    Self::send_desktop_notification("Yomi - Stopped", message);
                                    self.handle_streaming_error(
                                        StreamingStatus::Failed,
                                        message.to_string(),
                                    );
                                }
                                Some(FinishReason::ContentFilter) => {
                                    let message = " Response blocked: content filter triggered";
                                    Self::send_desktop_notification("Yomi - Stopped", message);
                                    self.handle_streaming_error(
                                        StreamingStatus::Failed,
                                        message.to_string(),
                                    );
                                }
                                _ => {
                                    // Normal completion
                                    self.finalize_assistant_message();
                                    self.stop_streaming(StreamingStatus::Completed);
                                    let message = format!(
                                        "😸 Task completed ({total_iterations} iterations)"
                                    );
                                    Self::send_desktop_notification("Yomi", &message);
                                    self.show_notification(
                                        &crate::components::info_bar::Notification::success(
                                            &message, 5000,
                                        ),
                                    );
                                }
                            }
                            self.state.should_redraw = true;
                        }
                        AgentStatus::Stopped { reason } => match reason {
                            StopReason::Completed => {
                                // Goal-mode completion skips TurnCompleted, so ensure cleanup
                                // happens here if streaming is still active.
                                if self.state.is_streaming {
                                    self.finalize_assistant_message();
                                    self.stop_streaming(StreamingStatus::Completed);
                                    self.state.should_redraw = true;
                                }
                                let message = "🎯 Goal completed";
                                Self::send_desktop_notification("Yomi", message);
                                self.show_notification(
                                    &crate::components::info_bar::Notification::success(
                                        message, 5000,
                                    ),
                                );
                            }
                            StopReason::Cancelled { operation } => {
                                // Cancelled - no desktop notification, just update UI
                                let message = operation.map_or_else(
                                    || " Cancelled".to_string(),
                                    |op| format!(" Cancelled: {op}"),
                                );
                                self.handle_streaming_error(StreamingStatus::Cancelled, message);
                            }
                            StopReason::Failed { error } => {
                                let message = format!(" Task failed: {error}");
                                Self::send_desktop_notification("Yomi - Error", &message);
                                self.handle_streaming_error(
                                    StreamingStatus::Failed,
                                    format!("Agent error: {error}"),
                                );
                            }
                            StopReason::MaxIterations { reached } => {
                                let message = format!(" Max iterations reached ({reached})");
                                Self::send_desktop_notification("Yomi - Stopped", &message);
                                self.handle_streaming_error(
                                    StreamingStatus::MaxIterations,
                                    format!("Reached maximum iterations ({reached})"),
                                );
                            }
                        },
                    }
                }
                // Error events - recoverable or non-recoverable
                Event::Agent(kernel::event::AgentEvent::Error {
                    phase,
                    error,
                    is_recoverable,
                    ..
                }) => {
                    let phase_str = format!("{phase:?}");
                    if is_recoverable {
                        // Recoverable error: show in status bar with warning color
                        let message = format!("{phase_str} error (will retry): {error}");
                        self.show_notification(&crate::components::info_bar::Notification::warn(
                            message, 3000,
                        ));
                        self.state.should_redraw = true;
                    } else {
                        // Non-recoverable error: stop streaming and add to chat view
                        self.handle_streaming_error(
                            StreamingStatus::Failed,
                            format!("{phase_str} error: {error}"),
                        );
                    }
                }
                // Retrying event - show in status bar
                Event::Agent(kernel::event::AgentEvent::Retrying {
                    attempt,
                    max_attempts,
                    reason,
                    ..
                }) => {
                    let message = format!("Retrying ({attempt}/{max_attempts}): {reason}");
                    // 0 = no timeout, persists until cleared
                    self.show_notification(&crate::components::info_bar::Notification::info(
                        message, 0,
                    ));
                    self.state.should_redraw = true;
                }
                // Session shutdown - only handle error cases (normal completion handled by ReActLoopEnd)
                Event::System(kernel::event::SystemEvent::Shutdown {
                    error: Some(err), ..
                }) => {
                    self.handle_streaming_error(
                        StreamingStatus::Failed,
                        format!("Session closed with error: {err}"),
                    );
                    self.state.should_redraw = true;
                }
                // Rewind completed - refresh messages from the event
                Event::System(kernel::event::SystemEvent::Rewound { messages, .. }) => {
                    // Recalculate token usage first (before moving messages)
                    let context_window = crate::config().agent.compactor.context_window;
                    let total_tokens: u32 = messages
                        .iter()
                        .filter_map(|m| m.token_usage.map(|u| u.total_tokens))
                        .next_back()
                        .unwrap_or_else(|| {
                            use kernel::utils::tokens;
                            messages
                                .iter()
                                .map(|m| tokens::estimate_tokens(&m.text_content()))
                                .sum::<usize>() as u32
                        });

                    // Refresh chat view with updated messages (truncate to before checkpoint)
                    // Note: We use CLEAR_HISTORY + INIT_HISTORY because there's no truncate API
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::CLEAR_HISTORY),
                        AttrValue::Flag(true),
                    );

                    if !messages.is_empty() {
                        // Pass Vec<Arc<Message>> directly - avoids cloning Message content
                        let _ = self.app.attr(
                            &Id::ChatView,
                            Attribute::Custom(attr::INIT_HISTORY),
                            AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(
                                messages,
                            ))),
                        );
                    }

                    // Update token usage in status bar
                    let usage_str = format!("{total_tokens}\x00{context_window}");
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom(attr::SET_CTX_USAGE),
                        AttrValue::String(usage_str),
                    );

                    self.show_notification(&crate::components::info_bar::Notification::success(
                        "Rewound to checkpoint",
                        3000,
                    ));
                    self.state.should_redraw = true;
                }
                // Note: StateChanged is currently ignored to avoid UI noise
                // Could be shown in status bar for debugging if needed
                Event::Agent(kernel::event::AgentEvent::PermissionRequest {
                    req_id,
                    tool_name,
                    tool_args,
                    tool_level,
                    ..
                }) => {
                    tracing::info!(
                        "TUI received PermissionRequest: {} for {}",
                        req_id,
                        tool_name
                    );
                    // Store the pending permission request
                    self.pending_permission = Some(req_id.clone());

                    // Show confirmation dialog with "Always approve" and "YOLO" options
                    let message =
                        format!("Tool: {tool_name}\nLevel: {tool_level}\nArgs: {tool_args}");
                    let dialog_data = format!(
                       "Can I run this tool?\x00Sure\x00Always allow this tool with level {tool_level}\x00Not now\x00YOLO - allow all dangerous tools\x00{message}"
                    );
                    tracing::debug!("Showing dialog with data: {dialog_data}",);
                    let _ = self.app.attr(
                        &Id::Dialog,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::String(dialog_data),
                    );
                    // Give focus to dialog so it receives keyboard events
                    self.set_focus(&Id::Dialog);
                    tracing::debug!("Dialog focused");
                    self.state.should_redraw = true;
                }
                _ => {}
            }
        }
        Ok(())
    }
}
