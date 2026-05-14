//! Streaming-related methods for Model

use tuirealm::props::{AttrValue, Attribute};

use crate::{attr, id::Id};

use super::types::{Model, StreamingStatus};

impl Model {
    /// Start streaming - initialize UI components for streaming state
    pub(crate) fn start_streaming(&mut self) {
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

    /// Clear the tool call delta display from `InfoBar`.
    pub(crate) fn clear_tool_call_delta(&mut self) {
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr::CLEAR_TOOL_CALL),
            AttrValue::Flag(true),
        );
    }

    /// Stop streaming with given status - cleanup UI and save content
    pub(crate) fn stop_streaming(&mut self, status: StreamingStatus) {
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

    /// Append streaming content to `ChatView` and `InfoBar`
    pub(crate) fn append_streaming_content(&mut self, text: &str, is_thinking: bool) {
        use std::time::Instant;

        if is_thinking {
            if self.thinking_start_time.is_none() {
                self.thinking_start_time = Some(Instant::now());
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
        let attr = if is_thinking {
            "append_thinking"
        } else {
            "append_content"
        };
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr),
            AttrValue::String(text.to_string()),
        );
        self.state.should_redraw = true;
    }

    /// Save assistant message to chat history and clear streaming
    pub(crate) fn finalize_assistant_message(&mut self) {
        if let Some(combined) = self.build_assistant_payload() {
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
}
