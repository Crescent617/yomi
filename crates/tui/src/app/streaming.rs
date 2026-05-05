//! Streaming content handling

use tuirealm::props::{AttrValue, Attribute};

use crate::app::state::StreamingStatus;
use crate::app::types::Model;
use crate::attr;
use crate::id::Id;

/// Streaming operations trait
pub trait StreamingOps {
    fn start_streaming(&mut self);
    fn stop_streaming(&mut self, status: StreamingStatus);
    fn append_streaming_content(&mut self, text: &str, is_thinking: bool);
    fn clear_streaming_state(&mut self);
    fn finalize_assistant_message(&mut self);
    fn save_partial_content(&mut self) -> anyhow::Result<()>;
    fn clear_tool_call_delta(&mut self);
}

impl StreamingOps for Model {
    fn start_streaming(&mut self) {
        self.state.is_streaming = true;
        self.clear_streaming_state();
        // Start ChatView streaming
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::START_STREAMING),
            AttrValue::Flag(true),
        );
        // Start InfoBar streaming
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr::START_STREAMING),
            AttrValue::Flag(true),
        );
        self.state.should_redraw = true;
    }

    fn stop_streaming(&mut self, status: StreamingStatus) {
        self.state.is_streaming = false;

        // Clear tool call state
        self.clear_tool_call_delta();

        match status {
            StreamingStatus::Completed => {
                let _ = self.app.attr(
                    &Id::InfoBar,
                    Attribute::Custom(attr::STOP_STREAMING),
                    AttrValue::Flag(true),
                );
                let _ = self.app.attr(
                    &Id::StatusBar,
                    Attribute::Custom(attr::CLEAR_MESSAGE),
                    AttrValue::Flag(true),
                );
                let _ = self.app.attr(
                    &Id::ChatView,
                    Attribute::Custom(attr::STOP_STREAMING),
                    AttrValue::Flag(true),
                );
                // Send queued message if any
                self.send_queued_message();
            }
            StreamingStatus::Cancelled
            | StreamingStatus::Failed
            | StreamingStatus::MaxIterations => {
                let _ = self.save_partial_content();
                let _ = self.app.attr(
                    &Id::InfoBar,
                    Attribute::Custom(attr::CANCEL_STREAMING),
                    AttrValue::Flag(true),
                );
                let _ = self.app.attr(
                    &Id::ChatView,
                    Attribute::Custom(attr::CANCEL_STREAMING),
                    AttrValue::Flag(true),
                );
                // Clear queued message on interruption
                self.clear_queued_message();
            }
        }
        self.clear_streaming_state();
        self.state.should_redraw = true;
    }

    fn append_streaming_content(&mut self, text: &str, is_thinking: bool) {
        if is_thinking {
            if self.thinking_start_time.is_none() {
                self.thinking_start_time = Some(std::time::Instant::now());
            }
            self.current_thinking.push_str(text);
            let _ = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::APPEND_THINKING),
                AttrValue::String(text.to_string()),
            );
        } else {
            self.current_content.push_str(text);
            let _ = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::APPEND_CONTENT),
                AttrValue::String(text.to_string()),
            );
        }
        // Update InfoBar with content for token counting
        let attr_name = if is_thinking {
            "append_thinking"
        } else {
            "append_content"
        };
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr_name),
            AttrValue::String(text.to_string()),
        );
        self.state.should_redraw = true;
    }

    fn clear_streaming_state(&mut self) {
        self.current_content.clear();
        self.current_thinking.clear();
        self.thinking_start_time = None;
    }

    fn finalize_assistant_message(&mut self) {
        // Save if there's either content or thinking
        if !self.current_content.is_empty() || !self.current_thinking.is_empty() {
            let elapsed_ms = self
                .thinking_start_time
                .map(|start| start.elapsed().as_millis() as u64);

            let combined = if self.current_thinking.is_empty() {
                if let Some(ms) = elapsed_ms {
                    format!("{}\x00\x00{}", self.current_content, ms)
                } else {
                    self.current_content.clone()
                }
            } else {
                format!(
                    "{}\x00{}\x00{}",
                    self.current_content,
                    self.current_thinking,
                    elapsed_ms.unwrap_or(0)
                )
            };
            let _ = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::ADD_ASSISTANT_MSG),
                AttrValue::String(combined),
            );
        }
        // Clear streaming UI
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::CANCEL_STREAMING),
            AttrValue::Flag(true),
        );
    }

    fn save_partial_content(&mut self) -> anyhow::Result<()> {
        if !self.current_content.is_empty() || !self.current_thinking.is_empty() {
            let elapsed_ms = self
                .thinking_start_time
                .map(|start| start.elapsed().as_millis() as u64);

            let combined = if self.current_thinking.is_empty() {
                if let Some(ms) = elapsed_ms {
                    format!("{}\x00\x00{}", self.current_content, ms)
                } else {
                    self.current_content.clone()
                }
            } else {
                format!(
                    "{}\x00{}\x00{}",
                    self.current_content,
                    self.current_thinking,
                    elapsed_ms.unwrap_or(0)
                )
            };
            self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::ADD_ASSISTANT_MSG),
                AttrValue::String(combined),
            )?;
        }
        Ok(())
    }

    fn clear_tool_call_delta(&mut self) {
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr::CLEAR_TOOL_CALL),
            AttrValue::Flag(true),
        );
    }
}

/// Queued message operations
pub trait QueuedMessageOps {
    fn set_queued_message(&mut self, blocks: Vec<kernel::types::ContentBlock>);
    fn clear_queued_message(&mut self);
    fn send_queued_message(&mut self) -> bool;
}

impl QueuedMessageOps for Model {
    fn set_queued_message(&mut self, blocks: Vec<kernel::types::ContentBlock>) {
        // Check if there's already a queued message
        if self.queued_message.is_some() {
            tracing::info!("Overwriting existing queued message with new one");
        }
        // Serialize the queued message for display in ChatView
        let blocks_json = serde_json::to_string(&blocks).unwrap_or_default();
        if let Err(e) = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::SET_QUEUED_MESSAGE),
            AttrValue::String(blocks_json),
        ) {
            tracing::warn!("Failed to set queued message in ChatView: {}", e);
        }
        self.queued_message = Some(blocks);
        self.state.should_redraw = true;
    }

    fn clear_queued_message(&mut self) {
        if let Err(e) = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::CLEAR_QUEUED_MESSAGE),
            AttrValue::Flag(true),
        ) {
            tracing::warn!("Failed to clear queued message in ChatView: {}", e);
        }
        self.queued_message = None;
        self.state.should_redraw = true;
    }

    fn send_queued_message(&mut self) -> bool {
        if let Some(blocks) = self.queued_message.take() {
            // Clear the queued message display in ChatView
            if let Err(e) = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::CLEAR_QUEUED_MESSAGE),
                AttrValue::Flag(true),
            ) {
                tracing::warn!("Failed to clear queued message in ChatView: {}", e);
            }
            // Send to kernel (streaming will be started by ModelEvent::Request)
            if let Err(e) = self.input_tx.try_send(blocks) {
                tracing::error!("Failed to send queued message to kernel: {}", e);
            }
            self.state.should_redraw = true;
            true
        } else {
            false
        }
    }
}
