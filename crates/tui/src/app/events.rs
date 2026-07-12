//! Kernel event processing

use tuirealm::props::{AttrValue, Attribute};
#[cfg(windows)]
use tuirealm::terminal::TerminalAdapter;

use crate::{attr, id::Id};
use kernel::event::{AgentStatus, Event, StopReason};
use kernel::tools::TODO_TOOL_NAME;
use kernel::types::FinishReason;
use std::sync::Arc;

use super::types::{Model, StreamingStatus};

impl Model {
    /// Caps processing time to ~8ms per frame to avoid UI stalls when
    /// a large batch of events arrives over IPC.
    pub async fn process_kernel_event(&mut self) {
        use super::event_pump::TaggedEvent;
        use tokio::sync::mpsc::error::TryRecvError;

        let start = std::time::Instant::now();
        loop {
            let event = match self.event_rx.try_recv() {
                Ok(TaggedEvent::Main(ev)) => ev,
                Ok(TaggedEvent::Connected) => {
                    self.show_notification(&crate::components::info_bar::Notification::info(
                        "Connected to daemon",
                        3000,
                    ));
                    self.state.should_redraw = true;
                    continue;
                }
                Ok(TaggedEvent::ConnectionLost) => {
                    self.show_notification(&crate::components::info_bar::Notification::warn(
                        "Connection lost, reconnecting…",
                        0,
                    ));
                    self.state.should_redraw = true;
                    continue;
                }
                Ok(TaggedEvent::Subagent {
                    parent_tool_id,
                    session_id,
                    event: ev,
                }) => {
                    tracing::debug!(
                        "Processing subagent event for {} ({}): {:?}",
                        parent_tool_id,
                        session_id,
                        std::mem::discriminant(&ev)
                    );
                    self.handle_subagent_event(&parent_tool_id, &session_id, &ev)
                        .await;
                    continue;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    tracing::error!("EventPump disconnected");
                    break;
                }
            };

            tracing::debug!(
                "Processing main event: {:?}",
                std::mem::discriminant(&event)
            );

            match event {
                // User message from kernel (render after kernel accepts it)
                Event::User(kernel::event::UserEvent::Message { content, .. }) => {
                    let blocks_json = serde_json::to_string(&content).unwrap_or_default();
                    if let Err(e) = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::ADD_USER_MESSAGE),
                        AttrValue::String(blocks_json),
                    ) {
                        tracing::error!("Failed to add user message to ChatView: {}", e);
                    } else {
                        self.scroll_chat_to_bottom();
                        self.state.should_redraw = true;
                    }
                }
                Event::User(kernel::event::UserEvent::Steer { content, .. }) => {
                    let blocks_json = serde_json::to_string(&content).unwrap_or_default();
                    if let Err(e) = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::ADD_STEER_MESSAGE),
                        AttrValue::String(blocks_json),
                    ) {
                        tracing::error!("Failed to add steer message to ChatView: {}", e);
                    } else {
                        self.scroll_chat_to_bottom();
                        self.state.should_redraw = true;
                    }
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
                    if let Err(e) = self.app.attr(&Id::InfoBar, attr, value) {
                        tracing::warn!("Failed to append tool call delta: {e}");
                    }
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
                    if let Err(e) = self.app.attr(&Id::InfoBar, attr, AttrValue::Flag(active)) {
                        tracing::warn!("Failed to update compacting status: {e}");
                    }
                    self.state.should_redraw = true;
                }
                Event::Model(kernel::event::ModelEvent::TokenUsage {
                    total_tokens,
                    context_window,
                    ..
                }) => {
                    // Keep the cached context window in sync with what the
                    // kernel reports (the session model may have been switched
                    // elsewhere, e.g. via the GUI; there is no dedicated
                    // model-changed event carrying the model name yet, so
                    // `model_name` may still go stale until restart).
                    if context_window > 0 {
                        self.context_window = context_window;
                    }
                    // Update context window usage in status bar
                    let usage_str = format!("{total_tokens}\x00{context_window}");
                    if let Err(e) = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom(attr::SET_CTX_USAGE),
                        AttrValue::String(usage_str),
                    ) {
                        tracing::warn!("Failed to update token usage: {e}");
                    }
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
                    if let Err(e) = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::START_TOOL),
                        AttrValue::String(combined),
                    ) {
                        tracing::warn!("Failed to show tool start: {e}");
                    }
                    self.state.should_redraw = true;
                }
                Event::Tool(kernel::event::ToolEvent::Metadata {
                    tool_id, metadata, ..
                }) => {
                    if let Some(subagent_sid) = metadata.get("subagent_session_id") {
                        let description = metadata
                            .get("subagent_description")
                            .cloned()
                            .unwrap_or_default();
                        let payload = format!("{tool_id}\x00{subagent_sid}\x00{description}");
                        if let Err(e) = self.app.attr(
                            &Id::ChatView,
                            Attribute::Custom(attr::INIT_SUBAGENT),
                            AttrValue::String(payload),
                        ) {
                            tracing::warn!("Failed to init subagent: {e}");
                        }
                        self.state.should_redraw = true;
                    }
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
                        if let Err(e) = self.app.attr(
                            &Id::ChatView,
                            Attribute::Custom(attr::FAIL_TOOL),
                            AttrValue::String(combined),
                        ) {
                            tracing::warn!("Failed to show tool error: {e}");
                        }
                    } else {
                        // Show tool output in chat view
                        // Format: tool_id\x00output\x00elapsed_ms\x00content_blocks_json
                        let blocks_json =
                            serde_json::to_string(&content_blocks).unwrap_or_default();
                        let combined =
                            format!("{tool_id}\x00{output}\x00{elapsed_ms}\x00{blocks_json}");
                        if let Err(e) = self.app.attr(
                            &Id::ChatView,
                            Attribute::Custom(attr::COMPLETE_TOOL),
                            AttrValue::String(combined),
                        ) {
                            tracing::warn!("Failed to show tool output: {e}");
                        }

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
                // Agent lifecycle state changes
                Event::Agent(kernel::event::AgentEvent::Lifecycle { state, .. }) => {
                    match state {
                        AgentStatus::Running => {
                            // Agent started - could show in status bar if needed
                        }
                        AgentStatus::Stopped { reason } => match reason {
                            StopReason::Completed { finish_reason } => {
                                match finish_reason {
                                    Some(FinishReason::MaxTokens) => {
                                        self.stop_streaming(StreamingStatus::Failed);
                                        self.show_error_message(
                                            "Response truncated: max tokens reached",
                                        );
                                    }
                                    Some(FinishReason::ContentFilter) => {
                                        self.stop_streaming(StreamingStatus::Failed);
                                        self.show_error_message(
                                            "Response blocked: content filter triggered",
                                        );
                                    }
                                    _ => {
                                        if self.state.is_streaming {
                                            self.finalize_assistant_message();
                                            self.stop_streaming(StreamingStatus::Completed);
                                        }
                                    }
                                }
                                let message = "😸 Task completed";
                                Self::send_desktop_notification("Yomi", message);
                                self.show_notification(
                                    &crate::components::info_bar::Notification::success(
                                        message, 5000,
                                    ),
                                );
                                self.state.should_redraw = true;
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
                        // Non-recoverable error: show notification only.
                        // Streaming ends only when AgentStatus::Stopped arrives.
                        let message = format!("{phase_str} error: {error}");
                        self.show_notification(&crate::components::info_bar::Notification::error(
                            message, 5000,
                        ));
                        self.state.should_redraw = true;
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
                Event::Agent(kernel::event::AgentEvent::GoalUpdated {
                    description,
                    status,
                }) => {
                    let goal_str = format!("{status}\x00{description}");
                    if let Err(e) = self.app.attr(
                        &Id::TodoList,
                        Attribute::Custom(attr::SET_GOAL),
                        AttrValue::String(goal_str),
                    ) {
                        tracing::warn!("Failed to update goal status on TodoList: {e}");
                    }
                    self.show_notification(&crate::components::info_bar::Notification::info(
                        format!("Goal {status}: {description}"),
                        3000,
                    ));
                    self.state.should_redraw = true;
                }
                Event::Agent(kernel::event::AgentEvent::GoalStopped) => {
                    if let Err(e) = self.app.attr(
                        &Id::TodoList,
                        Attribute::Custom(attr::SET_GOAL),
                        AttrValue::String(String::new()),
                    ) {
                        tracing::warn!("Failed to clear goal status on TodoList: {e}");
                    }
                    self.show_notification(&crate::components::info_bar::Notification::info(
                        "Goal stopped",
                        3000,
                    ));
                    self.state.should_redraw = true;
                }
                // Messages were replaced wholesale (rewind/undo, /clear, or
                // compaction) — reload the full history from the kernel.
                Event::Agent(kernel::event::AgentEvent::MessageReplaced { .. }) => {
                    let sid = kernel::types::SessionId::from(self.session_id.clone());
                    match self.kernel.list_messages(&sid).await {
                        Ok(session_messages) => {
                            let context_window = self.context_window;
                            let total_tokens = crate::app::calc_token_usage(&session_messages);

                            let _ = self.app.attr(
                                &Id::ChatView,
                                Attribute::Custom(attr::CLEAR_HISTORY),
                                AttrValue::Flag(true),
                            );

                            if !session_messages.is_empty() {
                                let _ = self.app.attr(
                                    &Id::ChatView,
                                    Attribute::Custom(attr::INIT_HISTORY),
                                    AttrValue::Payload(tuirealm::props::PropPayload::Any(
                                        Box::new(session_messages),
                                    )),
                                );
                            }

                            let usage_str = format!("{total_tokens}\x00{context_window}");
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::SET_CTX_USAGE),
                                AttrValue::String(usage_str),
                            );

                            // User-visible confirmation (replaces the old
                            // AgentEvent::Rewound "Rewound to checkpoint" toast).
                            // MessageReplaced also fires for /clear and compaction,
                            // so use wording that is accurate for all cases.
                            self.show_notification(
                                &crate::components::info_bar::Notification::success(
                                    "Conversation history updated",
                                    3000,
                                ),
                            );
                        }
                        Err(e) => {
                            tracing::warn!("Failed to reload messages after MessageReplaced: {e}");
                        }
                    }
                    self.state.should_redraw = true;
                }
                Event::Agent(kernel::event::AgentEvent::AskUserQuestion {
                    req_id,
                    session_id,
                    questions,
                    ..
                }) => {
                    tracing::info!(
                        "TUI received AskUserQuestion: {} with {} questions",
                        req_id,
                        questions.len()
                    );
                    Self::send_desktop_notification("Yomi", "Agent has a question for you");

                    // Auto-deny any previous pending ask-user request
                    if let Some((old_req_id, old_session_id, _, _)) = self.pending_ask_user.take() {
                        tracing::warn!(
                            "Auto-denying stale ask_user request {} (new: {})",
                            old_req_id,
                            req_id
                        );
                        let coord = Arc::clone(&self.kernel);
                        let sid = kernel::types::SessionId::from(old_session_id);
                        tokio::spawn(async move {
                            let response = kernel::tools::AskUserResponse {
                                answers: std::collections::HashMap::new(),
                            };
                            if let Err(e) = coord
                                .send_ask_user_response(&sid, &old_req_id, response)
                                .await
                            {
                                tracing::error!(
                                    "Failed to send auto-deny ask_user response: {}",
                                    e
                                );
                            }
                        });
                    }

                    // Store the request and show the first question
                    let first = questions.first().cloned();
                    self.pending_ask_user = Some((
                        req_id,
                        session_id.clone(),
                        std::collections::VecDeque::from(questions),
                        std::collections::HashMap::new(),
                    ));

                    if let Some(q) = first {
                        self.show_ask_user_question(&q);
                    }
                    self.state.should_redraw = true;
                }
                Event::Agent(kernel::event::AgentEvent::PermissionRequest {
                    req_id,
                    session_id,
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
                    // If a previous permission request is still pending, auto-deny it
                    // so the server doesn't hang waiting for a response.
                    if let Some((old_id, old_session_id)) = self.pending_permission.take() {
                        tracing::warn!(
                            "Auto-denying stale permission request {} (new: {})",
                            old_id,
                            req_id
                        );
                        let coord = Arc::clone(&self.kernel);
                        let sid = kernel::types::SessionId::from(old_session_id);
                        tokio::spawn(async move {
                            if let Err(e) = coord
                                .send_permission_response(&sid, &old_id, false, false)
                                .await
                            {
                                tracing::error!(
                                    "Failed to send auto-deny permission response: {}",
                                    e
                                );
                            }
                        });
                    }
                    self.pending_permission = Some((req_id, session_id.clone()));

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
                Event::Agent(kernel::event::AgentEvent::PermissionAck { req_id }) => {
                    if let Some((old_id, _)) = self.pending_permission.as_ref() {
                        if old_id == &req_id {
                            self.pending_permission = None;
                            // Close the dialog and restore focus immediately so the user
                            // never ends up typing into a stale / invisible dialog.
                            let _ = self.app.attr(
                                &Id::Dialog,
                                Attribute::Custom(attr::DIALOG_HIDE),
                                AttrValue::Flag(true),
                            );
                            self.set_focus(&Id::InputBox);
                            self.state.should_redraw = true;
                        }
                    }
                }
                Event::Agent(kernel::event::AgentEvent::AskUserAck { req_id }) => {
                    if let Some((old_id, _, _, _)) = self.pending_ask_user.as_ref() {
                        if old_id == &req_id {
                            self.pending_ask_user = None;
                            let _ = self.app.attr(
                                &Id::Dialog,
                                Attribute::Custom(attr::DIALOG_HIDE),
                                AttrValue::Flag(true),
                            );
                            self.set_focus(&Id::InputBox);
                            self.state.should_redraw = true;
                        }
                    }
                }
                _ => {}
            }
            // Cap event processing time to keep UI responsive (~60fps budget)
            if start.elapsed()
                > std::time::Duration::from_millis(crate::app::types::FRAME_BUDGET_MS)
            {
                break;
            }
        }
    }

    /// Handle events coming from a subagent session.
    ///
    /// These events are associated with a specific `Agent` tool call via
    /// `parent_tool_id`.  The UI can use them to show real-time progress
    /// inside that tool call's card.
    async fn handle_subagent_event(
        &mut self,
        parent_tool_id: &str,
        _session_id: &str,
        event: &Event,
    ) {
        use tuirealm::props::{AttrValue, Attribute};

        let is_stopped = matches!(
            event,
            Event::Agent(kernel::event::AgentEvent::Lifecycle {
                state: kernel::event::AgentStatus::Stopped { .. },
                ..
            })
        );

        let event_json = serde_json::to_string(event).unwrap_or_default();
        let payload = format!("{parent_tool_id}\x00{event_json}");
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::UPDATE_SUBAGENT),
            AttrValue::String(payload),
        );

        if is_stopped {
            let _ = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::FINALIZE_SUBAGENT),
                AttrValue::String(parent_tool_id.to_string()),
            );
        }
    }
}
