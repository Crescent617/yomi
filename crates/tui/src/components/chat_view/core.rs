//! Unified chat view component
//!
//! Displays chat history + streaming message in a single scrollable view.

use std::sync::Arc;

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::{Event, Key, KeyEvent, KeyModifiers},
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{layout::Rect, style::Style, text::Line, Frame},
    state::State,
};

use crate::components::chat_view::message_renderer::{
    get_message_pretty_json, get_message_raw_content, render_message, render_pending_items,
    render_thinking_lines,
};
use crate::components::chat_view::{
    line_display_width, scan_code_blocks, CodeBlockOverlay, CodeBlockOverlayManager, ContextMenu,
    ContextMenuAction,
};
use crate::components::wrap_info::WrapInfo;
use crate::components::wrap_paragraph::WrapParagraph;

use crate::{
    attr, components::info_bar::Notification, markdown_stream::StreamingMarkdownRenderer, msg::Msg,
    theme::colors, utils::text::substring_by_chars,
};

use kernel::types::{ContentBlock, ToolOutputBlock};

/// Tool execution status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Subagent execution status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Real-time state of a running subagent, displayed inline inside its parent tool card.
#[derive(Debug, Clone)]
pub struct SubagentState {
    pub session_id: String,
    pub description: String,
    pub status: SubagentStatus,
    /// Accumulated events from the subagent (chunks, tool calls, lifecycle).
    pub events: Vec<kernel::event::Event>,
    /// Whether the inline detail view is folded.
    pub folded: bool,
    /// Accumulated prompt tokens across all `TokenUsage` events.
    pub total_prompt_tokens: u32,
    /// Accumulated completion tokens across all `TokenUsage` events.
    pub total_completion_tokens: u32,
}

/// Result of handling a mouse event
#[derive(Debug)]
pub enum MouseAction {
    /// Selection was copied to clipboard
    Copied(String),
    /// Scroll-to-bottom button was clicked
    ScrollToBottom,
    /// Code block was copied
    CodeCopied,
    /// Context menu action completed
    MenuAction(String),
    /// No action taken
    None,
}

/// A chat message in history
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum HistoryMessage {
    User(Vec<ContentBlock>),
    Steer(Vec<ContentBlock>),
    Assistant {
        content: String,
        thinking: Option<String>,
        thinking_folded: bool,
        thinking_elapsed_ms: Option<u64>,
    },
    Tool {
        tool_name: String,
        tool_id: String,
        status: ToolStatus,
        output: Option<String>,
        error: Option<String>,
        folded: bool,
        arguments: Option<String>,
        elapsed_ms: Option<u64>,
        content_blocks: Vec<ToolOutputBlock>,
        /// Real-time subagent progress, if this tool is an `agent` call.
        subagent: Option<SubagentState>,
    },
    Error(String),
    /// UI notice (e.g. reconnected to daemon). Not an LLM message role.
    Notice(String),
}

/// Text selection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}

impl Selection {
    /// Get normalized selection (start <= end)
    #[must_use]
    pub fn normalized(&self) -> Self {
        if self.start_line < self.end_line
            || (self.start_line == self.end_line && self.start_col <= self.end_col)
        {
            *self
        } else {
            Self {
                start_line: self.end_line,
                start_col: self.end_col,
                end_line: self.start_line,
                end_col: self.start_col,
            }
        }
    }

    /// Check if a position is within the selection
    pub fn contains(&self, line: usize, col: usize) -> bool {
        let norm = self.normalized();
        (line > norm.start_line || (line == norm.start_line && col >= norm.start_col))
            && (line < norm.end_line || (line == norm.end_line && col <= norm.end_col))
    }
}

/// Unified chat view component
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct ChatView {
    props: Props,
    messages: Vec<HistoryMessage>,
    scroll_offset: usize,
    // Streaming state
    streaming_content: String,
    streaming_thinking: String,
    is_streaming: bool,
    md_renderer: StreamingMarkdownRenderer,

    // Expand all mode (ctrl-o): show all thinking and tool details
    expand_all: bool,
    // Pending mailbox items displayed at the bottom (steer + queued)
    pending_items: Vec<kernel::comms::MailboxItem>,
    // Track visible height for scroll calculations
    last_visible_height: usize,
    // Per-message rendered lines. Replaced on invalidate, pushed on new msg.
    msg_cache: Vec<Vec<Arc<Line<'static>>>>,
    // Flattened msg_cache + separators. Rebuilt when msg_cache_dirty.
    flat_lines: Vec<Arc<Line<'static>>>,
    // flat_lines + streaming + queued. Rebuilt every frame (clear+extend).
    all_lines: Vec<Arc<Line<'static>>>,
    // Width used for last render. If changed, msg_cache must be rebuilt.
    last_render_width: usize,
    msg_cache_dirty: bool,
    // Text selection state
    selection: Option<Selection>,
    is_selecting: bool,
    // Track last click for double-click detection
    last_click_time: Option<std::time::Instant>,
    last_click_pos: Option<(usize, usize)>,
    // Current display area for mouse coordinate conversion
    current_area: Option<Rect>,
    // Scroll-to-bottom button area for click detection
    scroll_button_area: Option<Rect>,
    // Wrap info cache with prefix sum and boundaries
    wrap_info: WrapInfo,
    // Copy button overlays for visible code blocks
    code_block_overlay_manager: CodeBlockOverlayManager,
    // Context menu for message actions (right-click)
    context_menu: Option<ContextMenu>,
}

impl Default for ChatView {
    fn default() -> Self {
        Self {
            props: Props::default(),
            messages: Vec::new(),
            scroll_offset: 0,
            streaming_content: String::new(),
            streaming_thinking: String::new(),
            is_streaming: false,
            md_renderer: StreamingMarkdownRenderer::new(),

            expand_all: false,
            pending_items: Vec::new(),
            last_visible_height: 0,
            msg_cache: Vec::new(),
            flat_lines: Vec::new(),
            all_lines: Vec::new(),
            last_render_width: 0,
            msg_cache_dirty: true,
            selection: None,
            is_selecting: false,
            last_click_time: None,
            last_click_pos: None,
            current_area: None,
            scroll_button_area: None,
            wrap_info: WrapInfo::new(),
            code_block_overlay_manager: CodeBlockOverlayManager::new(),
            context_menu: None,
        }
    }
}

impl ChatView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Invalidate cache for a specific message by index.
    fn invalidate_msg_cache(&mut self, idx: usize) {
        if idx < self.msg_cache.len() {
            let width = self.current_area.map_or(80, |a| a.width as usize);
            self.msg_cache[idx] = render_message(&self.messages[idx], width);
            self.msg_cache_dirty = true;
        }
    }

    /// Add a new empty cache entry for a new message.
    fn push_new_msg_cache(&mut self) {
        let width = self.current_area.map_or(80, |a| a.width as usize);
        let idx = self.messages.len() - 1;
        self.msg_cache
            .push(render_message(&self.messages[idx], width));
        self.msg_cache_dirty = true;
    }

    /// Clear all caches and messages.
    fn clear_all_caches(&mut self) {
        self.msg_cache.clear();
        self.flat_lines.clear();
        self.all_lines.clear();
        self.wrap_info.clear();
        self.selection = None;
        self.is_selecting = false;
    }

    /// Rebuild `msg_cache` for all messages (e.g. after width change or expand/collapse).
    fn rebuild_msg_cache(&mut self) {
        let width = self.current_area.map_or(80, |a| a.width as usize);
        self.msg_cache.clear();
        for msg in &self.messages {
            self.msg_cache.push(render_message(msg, width));
        }
        self.msg_cache_dirty = true;
    }

    /// Start text selection at the given position.
    pub fn start_selection(&mut self, line: usize, col: usize) {
        self.selection = Some(Selection {
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
        });
        self.is_selecting = true;
    }

    /// Update selection end position while dragging.
    pub fn update_selection(&mut self, line: usize, col: usize) {
        if let Some(ref mut sel) = self.selection {
            sel.end_line = line;
            sel.end_col = col;
        }
    }

    /// End text selection.
    pub fn end_selection(&mut self) {
        self.is_selecting = false;
    }

    /// Clear the current selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.is_selecting = false;
    }

    /// Set the pending mailbox items (steer + queued) to display.
    pub fn set_pending_items(&mut self, items: Vec<kernel::comms::MailboxItem>) {
        self.pending_items = items;
    }

    pub fn add_user_message(&mut self, content_blocks: Vec<ContentBlock>) {
        self.messages.push(HistoryMessage::User(content_blocks));
        self.push_new_msg_cache();
    }

    pub fn add_steer_message(&mut self, content_blocks: Vec<ContentBlock>) {
        self.flush_streaming();
        self.messages.push(HistoryMessage::Steer(content_blocks));
        self.push_new_msg_cache();
    }

    pub fn add_error_message(&mut self, error: String) {
        self.messages.push(HistoryMessage::Error(error));
        self.push_new_msg_cache();
    }

    pub fn add_notice(&mut self, text: String) {
        self.messages.push(HistoryMessage::Notice(text));
        self.push_new_msg_cache();
    }

    pub fn add_assistant_message(
        &mut self,
        content: String,
        thinking: Option<String>,
        elapsed_ms: Option<u64>,
    ) {
        self.messages.push(HistoryMessage::Assistant {
            content,
            thinking,
            thinking_folded: !self.expand_all,
            thinking_elapsed_ms: elapsed_ms,
        });
        self.push_new_msg_cache();
    }

    pub fn start_tool(&mut self, tool_id: String, tool_name: String, arguments: Option<String>) {
        // Flush any pending streaming content before starting tool
        self.flush_streaming();

        self.messages.push(HistoryMessage::Tool {
            tool_name,
            tool_id,
            status: ToolStatus::Running,
            output: None,
            error: None,
            folded: !self.expand_all,
            arguments,
            elapsed_ms: None,
            content_blocks: Vec::new(),
            subagent: None,
        });
        self.push_new_msg_cache();
    }

    /// Initialize a [`SubagentState`] on an existing `Agent` tool message.
    pub fn init_subagent(&mut self, parent_tool_id: &str, session_id: String, description: String) {
        for (i, msg) in self.messages.iter_mut().enumerate().rev() {
            if let HistoryMessage::Tool {
                tool_id, subagent, ..
            } = msg
            {
                if tool_id == parent_tool_id {
                    *subagent = Some(SubagentState {
                        session_id,
                        description,
                        status: SubagentStatus::Running,
                        events: Vec::new(),
                        folded: !self.expand_all,
                        total_prompt_tokens: 0,
                        total_completion_tokens: 0,
                    });
                    self.invalidate_msg_cache(i);
                    break;
                }
            }
        }
    }

    /// Append an event to an existing [`SubagentState`].
    pub fn update_subagent(&mut self, parent_tool_id: &str, event: kernel::event::Event) {
        for (i, msg) in self.messages.iter_mut().enumerate().rev() {
            if let HistoryMessage::Tool {
                tool_id,
                subagent: Some(ref mut sa),
                ..
            } = msg
            {
                if tool_id == parent_tool_id {
                    if let kernel::event::Event::Agent(kernel::event::AgentEvent::Lifecycle {
                        state: kernel::event::AgentStatus::Stopped { ref reason },
                        ..
                    }) = event
                    {
                        sa.status = match reason {
                            kernel::event::StopReason::Completed { .. } => {
                                SubagentStatus::Completed
                            }
                            kernel::event::StopReason::Cancelled { .. }
                            | kernel::event::StopReason::Shutdown => SubagentStatus::Cancelled,
                            _ => SubagentStatus::Failed,
                        };
                    }
                    if let kernel::event::Event::Model(kernel::event::ModelEvent::TokenUsage {
                        prompt_tokens,
                        completion_tokens,
                        ..
                    }) = event
                    {
                        sa.total_prompt_tokens += prompt_tokens;
                        sa.total_completion_tokens += completion_tokens;
                    }
                    sa.events.push(event);
                    self.invalidate_msg_cache(i);
                    break;
                }
            }
        }
    }

    /// Mark a subagent as finalized (when the parent `ToolEvent::End` arrives).
    pub fn finalize_subagent(&mut self, parent_tool_id: &str) {
        for msg in self.messages.iter_mut().rev() {
            if let HistoryMessage::Tool {
                tool_id,
                subagent: Some(ref mut sa),
                ..
            } = msg
            {
                if tool_id == parent_tool_id {
                    if matches!(sa.status, SubagentStatus::Running) {
                        sa.status = SubagentStatus::Completed;
                    }
                    break;
                }
            }
        }
    }

    pub fn complete_tool(
        &mut self,
        tool_id: &str,
        output: String,
        elapsed_ms: u64,
        content_blocks: Vec<ToolOutputBlock>,
    ) {
        // Update the tool message in history and invalidate cache
        for (i, msg) in self.messages.iter_mut().enumerate().rev() {
            if let HistoryMessage::Tool {
                tool_id: id,
                status,
                output: out,
                elapsed_ms: elapsed,
                content_blocks: blocks,
                ..
            } = msg
            {
                if id == tool_id {
                    *status = ToolStatus::Completed;
                    *out = Some(output);
                    *elapsed = Some(elapsed_ms);
                    *blocks = content_blocks;
                    self.invalidate_msg_cache(i);
                    break;
                }
            }
        }
    }

    pub fn fail_tool(&mut self, tool_id: &str, error: String, elapsed_ms: u64) {
        // Update the tool message in history and invalidate cache
        for (i, msg) in self.messages.iter_mut().enumerate().rev() {
            if let HistoryMessage::Tool {
                tool_id: id,
                status,
                error: err,
                elapsed_ms: elapsed,
                ..
            } = msg
            {
                if id == tool_id {
                    *status = ToolStatus::Failed;
                    *err = Some(error);
                    *elapsed = Some(elapsed_ms);
                    self.invalidate_msg_cache(i);
                    break;
                }
            }
        }
    }

    /// Flush pending streaming content to history.
    /// Called when a new block starts (tool, code block, etc.) to preserve current content.
    pub fn flush_streaming(&mut self) {
        let content = std::mem::take(&mut self.streaming_content);
        let thinking = std::mem::take(&mut self.streaming_thinking);
        let has_content = !content.is_empty();
        let has_thinking = !thinking.is_empty();
        if !has_content && !has_thinking {
            return;
        }

        self.messages.push(HistoryMessage::Assistant {
            content,
            thinking: has_thinking.then_some(thinking),
            thinking_folded: !has_thinking || !self.expand_all,
            thinking_elapsed_ms: None,
        });
        self.push_new_msg_cache();

        if has_content {
            self.md_renderer = StreamingMarkdownRenderer::new();
        }
    }

    pub fn start_streaming(&mut self) {
        self.is_streaming = true;
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        // Note: Don't reset scroll_offset here - respect user's scroll position
    }

    pub fn stop_streaming(&mut self) {
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.is_streaming = false;
    }

    /// Cancel streaming - flush partial content and mark running tools as cancelled
    pub fn cancel_streaming(&mut self) {
        // Note: Content is already saved by app.rs via add_assistant_with_thinking
        // Just clear streaming buffers without flushing to avoid duplicates
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.is_streaming = false;

        // Mark any running tools as cancelled and invalidate their caches.
        // Collect indices first (immutable borrow), then mutate.
        let running_indices: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter_map(|(i, msg)| match msg {
                HistoryMessage::Tool {
                    status: ToolStatus::Running,
                    ..
                } => Some(i),
                _ => None,
            })
            .collect();

        for idx in running_indices {
            if let Some(HistoryMessage::Tool { status, .. }) = self.messages.get_mut(idx) {
                *status = ToolStatus::Cancelled;
            }
            self.invalidate_msg_cache(idx);
        }
    }

    pub fn append_streaming_content(&mut self, text: &str) {
        self.streaming_content.push_str(text);
        self.md_renderer.append(text);
        // Note: streaming content is rendered separately, don't mark history cache dirty
        // The view() method handles streaming content independently
    }

    pub fn append_streaming_thinking(&mut self, text: &str) {
        self.streaming_thinking.push_str(text);
        // Note: streaming content is rendered separately, don't mark history cache dirty
    }

    /// Tick handler. The chat view has no per-frame animations (streaming
    /// status lives in the info bar shimmer), so it never requests redraws.
    pub const fn tick(&mut self) -> bool {
        false
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset += amount;
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    pub const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn scroll_to_top(&mut self) {
        // scroll_offset is now visual lines from bottom
        // To scroll to top, we need to set offset to total_visual_lines - visible_height
        let visible = self.last_visible_height.max(1);
        let total_visual = self.wrap_info.total_lines();
        self.scroll_offset = total_visual.saturating_sub(visible);
    }

    pub fn toggle_last_thinking(&mut self) {
        for (i, msg) in self.messages.iter_mut().enumerate().rev() {
            if let HistoryMessage::Assistant {
                thinking_folded, ..
            } = msg
            {
                *thinking_folded = !*thinking_folded;
                self.invalidate_msg_cache(i);
                break;
            }
        }
    }

    /// Check if user has scrolled up from the bottom
    /// Returns true if `scroll_offset > 0` (not at bottom)
    pub const fn is_scrolled_up(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Calculate the global visual line index at the top of the viewport.
    fn visual_scroll_top(&self) -> usize {
        self.wrap_info
            .total_lines()
            .saturating_sub(self.last_visible_height)
            .saturating_sub(self.scroll_offset)
    }

    /// Get scroll progress for browse mode (`current_line`, `total_lines`)
    /// Returns the 1-based current visible start position and total lines
    /// Note: now uses visual lines (post-wrap) instead of logical lines
    pub fn get_scroll_progress(&self) -> (usize, usize) {
        let total_visual = self.wrap_info.total_lines();
        if total_visual == 0 {
            return (0, 0);
        }

        // Calculate current visible start position (1-based)
        // scroll_offset = 0 means at bottom showing latest content
        // scroll_offset > 0 means scrolled up by that many visual lines from bottom
        let start_visual_line = if self.scroll_offset == 0 {
            // At bottom: show the last visible_height lines
            total_visual
                .saturating_sub(self.last_visible_height.saturating_sub(1))
                .max(1)
        } else {
            // Scrolled up: start_line is scroll_offset lines from bottom
            total_visual.saturating_sub(self.scroll_offset).max(1)
        };

        (start_visual_line.min(total_visual), total_visual)
    }

    /// Apply expand/collapse state to all messages and rebuild cache.
    fn apply_expand_all(&mut self, expand: bool) {
        self.expand_all = expand;
        for msg in &mut self.messages {
            match msg {
                HistoryMessage::Assistant {
                    thinking_folded, ..
                } => {
                    *thinking_folded = !expand;
                }
                HistoryMessage::Tool { folded, .. } => {
                    *folded = !expand;
                }
                _ => {}
            }
        }
        self.rebuild_msg_cache();
    }

    pub fn toggle_expand_all(&mut self) {
        self.apply_expand_all(!self.expand_all);
    }

    pub fn expand_all(&mut self) {
        if !self.expand_all {
            self.apply_expand_all(true);
        }
    }

    pub fn collapse_all(&mut self) {
        if self.expand_all {
            self.apply_expand_all(false);
        }
    }

    fn render_streaming(&mut self, width: usize) -> Vec<Arc<Line<'static>>> {
        let mut lines = Vec::new();

        // Render thinking if present (collapsed by default, expanded in expand_all mode)
        lines.extend(render_thinking_lines(
            &self.streaming_thinking,
            !self.expand_all,
            None,
            true,
            width,
        ));

        // Render content (no indicator, status shown in status bar)
        // Add separator between thinking and content
        if !self.streaming_thinking.is_empty() && !self.streaming_content.is_empty() {
            lines.push(Arc::new(Line::from("")));
        }
        self.md_renderer.set_width(width);
        let md_lines = self.md_renderer.lines();

        for line in md_lines {
            lines.push(Arc::new(line.clone()));
        }

        // Trailing separator to leave breathing room before queued message / next content
        lines.push(Arc::new(Line::from("")));
        lines
    }
}

impl ChatView {
    const MOUSE_SCROLL_LINES: usize = 2;

    /// Rebuild `flat_lines` from `msg_cache` if dirty.
    ///
    /// Flattens all `msg_cache` entries + separators into a single Vec.
    /// Returns `true` if the cache was actually rebuilt.
    fn rebuild_flat_lines(&mut self) -> bool {
        if !self.msg_cache_dirty {
            return false;
        }
        self.flat_lines.clear();
        for lines in &self.msg_cache {
            self.flat_lines.extend(lines.iter().cloned());
            self.flat_lines.push(Arc::new(Line::from("")));
        }
        self.msg_cache_dirty = false;
        true
    }

    /// Convert screen coordinates to line/column in visible content.
    fn screen_to_position(&self, mouse_x: u16, mouse_y: u16) -> Option<(usize, usize)> {
        let area = self.current_area?;

        // Check if click is within our area
        if mouse_x < area.x
            || mouse_x >= area.x + area.width
            || mouse_y < area.y
            || mouse_y >= area.y + area.height
        {
            return None;
        }

        let terminal_col = (mouse_x - area.x) as usize;
        let terminal_row = (mouse_y - area.y) as usize;

        let visual_scroll = self.visual_scroll_top();
        let target_visual_row = visual_scroll + terminal_row;

        let (logical_line, row_in_line) = self.wrap_info.visual_to_logical(target_visual_row);
        let line = self.all_lines.get(logical_line)?;
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let boundaries = self.wrap_info.get_boundaries(logical_line)?;

        let start_byte = boundaries.get(row_in_line).copied().unwrap_or(0);
        let end_byte = boundaries
            .get(row_in_line + 1)
            .copied()
            .unwrap_or(text.len());

        // display_col_to_char_idx returns a segment-local char index.
        // Add the prefix char count to get the global line index.
        let prefix_chars = text[..start_byte].chars().count();
        let segment_col = display_col_to_char_idx(&text, start_byte, end_byte, terminal_col);
        let char_col = prefix_chars + segment_col;
        Some((logical_line, char_col))
    }

    /// Extract selected text from all lines.
    fn get_selected_text(&self) -> Option<String> {
        let sel = self.selection?;
        tracing::debug!("get_selected_text: selection={:?}", sel);
        let norm = sel.normalized();
        tracing::debug!("get_selected_text: normalized={:?}", norm);

        // Check if selection is empty (start == end)
        if norm.start_line == norm.end_line && norm.start_col == norm.end_col {
            tracing::debug!("get_selected_text: empty selection!");
            return None;
        }

        let all_lines = &self.all_lines;
        tracing::debug!("get_selected_text: all_lines len={}", all_lines.len());
        let mut result = String::new();

        for (line_idx, line) in all_lines.iter().enumerate() {
            if line_idx < norm.start_line || line_idx > norm.end_line {
                continue;
            }

            let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let char_count = self.wrap_info.char_count(line_idx);

            if line_idx == norm.start_line && line_idx == norm.end_line {
                let start = norm.start_col.min(char_count);
                let end = norm.end_col.min(char_count);
                result.push_str(&substring_by_chars(&line_text, start, end));
            } else if line_idx == norm.start_line {
                let start = norm.start_col.min(char_count);
                result.push_str(&substring_by_chars(&line_text, start, char_count));
                result.push('\n');
            } else if line_idx == norm.end_line {
                let end = norm.end_col.min(char_count);
                result.push_str(&substring_by_chars(&line_text, 0, end));
            } else {
                result.push_str(&line_text);
                result.push('\n');
            }
        }

        tracing::debug!("get_selected_text: result len={}", result.len());
        Some(result)
    }

    /// Copy the current selection to clipboard.
    pub fn copy_selection(&self) -> Option<String> {
        let sel = self.selection?;
        tracing::debug!("copy_selection: selection={:?}", sel);

        let text = self.get_selected_text()?;
        tracing::debug!("copy_selection: got text len={}", text.len());

        if text.is_empty() {
            tracing::debug!("copy_selection: text is empty, returning None");
            return None;
        }

        // Copy to clipboard
        if let Err(e) = crate::utils::clipboard::copy_text(&text) {
            tracing::debug!("Failed to copy to clipboard: {}", e);
            return None;
        }

        tracing::debug!("copy_selection: success, text len={}", text.len());
        Some(text)
    }

    /// Check if this is a double click (within 300ms and same position).
    fn is_double_click(&mut self, line: usize, col: usize) -> bool {
        const DOUBLE_CLICK_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(300);

        let now = std::time::Instant::now();
        let is_double = self
            .last_click_time
            .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK_THRESHOLD)
            && self.last_click_pos == Some((line, col));

        self.last_click_time = Some(now);
        self.last_click_pos = Some((line, col));

        is_double
    }

    /// Select a word at the given position (double-click).
    fn select_word_at(&mut self, line: usize, col: usize) {
        let all_lines = &self.all_lines;
        if line >= all_lines.len() {
            return;
        }

        let line_text: String = all_lines[line]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();

        let char_count = line_text.chars().count();
        if col >= char_count {
            return;
        }

        // Find start of word: scan backwards from col to first non-word char
        let mut start_char_idx = 0;
        for (i, c) in line_text.chars().enumerate().take(col) {
            if !is_word_char(c) {
                start_char_idx = i + 1;
            }
        }

        // Find end of word: scan forwards from col to first non-word char
        let mut end_char_idx = char_count;
        for (i, c) in line_text.chars().enumerate().skip(col) {
            if !is_word_char(c) {
                end_char_idx = i;
                break;
            }
        }

        self.selection = Some(Selection {
            start_line: line,
            start_col: start_char_idx,
            end_line: line,
            end_col: end_char_idx,
        });
        self.is_selecting = true;
    }

    /// Check if a point is within the scroll-to-bottom button area.
    /// Also validates that the button area is within the current display area
    /// to prevent stale area bugs after window resize.
    fn is_click_on_scroll_button(&self, x: u16, y: u16) -> bool {
        let Some(button_area) = self.scroll_button_area else {
            return false;
        };
        let Some(current_area) = self.current_area else {
            return false;
        };

        // Validate button area is within current display bounds
        let button_in_bounds = button_area.x >= current_area.x
            && button_area.y >= current_area.y
            && button_area.x + button_area.width <= current_area.x + current_area.width
            && button_area.y + button_area.height <= current_area.y + current_area.height;

        if !button_in_bounds {
            return false;
        }

        x >= button_area.x
            && x < button_area.x + button_area.width
            && y >= button_area.y
            && y < button_area.y + button_area.height
    }

    /// Handle mouse event for text selection.
    /// Returns the action taken based on the mouse event.
    pub fn handle_mouse_event(
        &mut self,
        kind: tuirealm::event::MouseEventKind,
        x: u16,
        y: u16,
    ) -> MouseAction {
        use tuirealm::event::{MouseButton, MouseEventKind};

        // Handle context menu interactions first (if menu is open)
        if let Some(ref menu) = self.context_menu {
            match kind {
                MouseEventKind::Moved => {
                    // Update hover state
                    if let Some(ref mut menu) = self.context_menu {
                        menu.update_hover(x, y);
                    }
                    return MouseAction::None;
                }
                MouseEventKind::Down(_) => {
                    // Check if clicked outside menu - close it
                    if !menu.contains(x, y) {
                        self.context_menu = None;
                    }
                    // Clicked inside or outside menu - consume the event
                    return MouseAction::None;
                }
                MouseEventKind::Up(_) => {
                    // Check if an item was clicked
                    if let Some(action) = menu.get_action_at(x, y) {
                        let msg_idx = menu.message_idx;
                        self.context_menu = None;
                        return self.handle_context_menu_action(action, msg_idx);
                    }
                    // Clicked inside menu but not on an item - keep menu open
                    return MouseAction::None;
                }
                _ => return MouseAction::None,
            }
        }

        // Check if scroll button was clicked (on Down event)
        if matches!(kind, MouseEventKind::Down(_)) && self.is_click_on_scroll_button(x, y) {
            self.scroll_to_bottom();
            return MouseAction::ScrollToBottom;
        }

        // Check if code block copy button was clicked (on Down event)
        if matches!(kind, MouseEventKind::Down(_)) {
            if let Some(content) = self.code_block_overlay_manager.find_and_copy(x, y) {
                if let Err(e) = crate::utils::clipboard::copy_text(content) {
                    tracing::debug!("Failed to copy code block: {}", e);
                } else {
                    return MouseAction::CodeCopied;
                }
            }
        }

        // Get width from current area for coordinate conversion
        let _width = self.current_area.map_or(80, |a| a.width as usize);

        match kind {
            MouseEventKind::Down(MouseButton::Right) => {
                // Open context menu for message at position
                if let Some(msg_idx) = self.screen_to_message_index(x, y) {
                    if let Some(area) = self.current_area {
                        self.context_menu = ContextMenu::new(x, y, msg_idx, area);
                    }
                }
                MouseAction::None
            }
            MouseEventKind::Down(_) => {
                if let Some((line, col)) = self.screen_to_position(x, y) {
                    if self.is_double_click(line, col) {
                        // Rebuild caches before selecting word
                        self.rebuild_flat_lines();
                        self.select_word_at(line, col);
                    } else {
                        self.start_selection(line, col);
                    }
                    MouseAction::None
                } else {
                    self.clear_selection();
                    MouseAction::None
                }
            }
            MouseEventKind::Drag(_) => {
                if self.is_selecting {
                    if let Some((line, col)) = self.screen_to_position(x, y) {
                        self.update_selection(line, col);
                    }
                }
                MouseAction::None
            }
            MouseEventKind::Up(_) => {
                if self.is_selecting {
                    self.end_selection();
                    // Rebuild caches before copying to ensure we have all lines
                    self.rebuild_flat_lines();
                    // Auto-copy selection to clipboard when mouse is released
                    match self.copy_selection() {
                        Some(text) => MouseAction::Copied(text),
                        None => MouseAction::None,
                    }
                } else {
                    MouseAction::None
                }
            }
            _ => MouseAction::None,
        }
    }

    /// Handle a context menu action for a specific message
    fn handle_context_menu_action(&self, action: ContextMenuAction, msg_idx: usize) -> MouseAction {
        let msg = match self.messages.get(msg_idx) {
            Some(m) => m,
            None => return MouseAction::None,
        };

        match action {
            ContextMenuAction::CopyContent => {
                let content = get_message_raw_content(msg);
                if let Err(e) = crate::utils::clipboard::copy_text(&content) {
                    tracing::debug!("Failed to copy message content: {}", e);
                    MouseAction::None
                } else {
                    MouseAction::MenuAction("Message copied".to_string())
                }
            }
            ContextMenuAction::CopyPrettyJson => {
                let json = get_message_pretty_json(msg);
                if let Err(e) = crate::utils::clipboard::copy_text(&json) {
                    tracing::debug!("Failed to copy message JSON: {}", e);
                    MouseAction::None
                } else {
                    MouseAction::MenuAction("JSON copied".to_string())
                }
            }
        }
    }

    /// Convert screen coordinates to message index
    fn screen_to_message_index(&self, x: u16, y: u16) -> Option<usize> {
        let area = self.current_area?;

        // Check if click is within our area
        if x < area.x || x >= area.x + area.width || y < area.y || y >= area.y + area.height {
            return None;
        }

        let terminal_row = (y - area.y) as usize;
        let visual_scroll = self.visual_scroll_top();
        let target_visual_row = visual_scroll + terminal_row;

        let (logical_line, _) = self.wrap_info.visual_to_logical(target_visual_row);
        self.line_to_message_index(logical_line)
    }

    /// Convert a global line index to message index.
    /// Every message in `msg_cache` has a trailing separator line, including the last one.
    fn line_to_message_index(&self, line_idx: usize) -> Option<usize> {
        let mut current_line = 0;

        for (msg_idx, cache) in self.msg_cache.iter().enumerate() {
            let end = current_line + cache.len() + 1; // +1 trailing separator

            if line_idx >= current_line && line_idx < end {
                return Some(msg_idx);
            }

            current_line = end;
        }

        None
    }

    /// Extract raw text content from a `HistoryMessage`
    /// Draw scroll-to-bottom button at the bottom center
    fn draw_scroll_button(&mut self, frame: &mut Frame, area: Rect) {
        use tuirealm::ratatui::{
            layout::Alignment,
            widgets::{Clear, Paragraph},
        };

        const BUTTON_TEXT: &str = "↓ Bottom";
        const BUTTON_WIDTH: u16 = 10; // "↓ Bottom" = 8 chars + 2 padding
        const BUTTON_HEIGHT: u16 = 1;

        // Position button at bottom-center
        let button_x = area
            .x
            .saturating_add(area.width / 2)
            .saturating_sub(BUTTON_WIDTH / 2);
        let button_y = area
            .y
            .saturating_add(area.height)
            .saturating_sub(BUTTON_HEIGHT)
            .max(area.y);

        let button_area = Rect {
            x: button_x,
            y: button_y,
            width: BUTTON_WIDTH.min(area.width),
            height: BUTTON_HEIGHT.min(area.height),
        };

        // Store button area for click detection
        self.scroll_button_area = Some(button_area);

        // Clear the area behind the button
        frame.render_widget(Clear, button_area);

        // Render button with accent style
        let button_style = Style::default()
            .fg(colors::text_primary())
            .bg(colors::surface());

        let button = Paragraph::new(BUTTON_TEXT)
            .style(button_style)
            .alignment(Alignment::Center);

        frame.render_widget(button, button_area);
    }
}

impl ChatView {
    /// Calculate and render copy buttons for code blocks
    fn render_code_block_buttons(&mut self, frame: &mut Frame, area: Rect, visual_scroll: usize) {
        self.code_block_overlay_manager.clear();

        // Skip rendering copy buttons during streaming to avoid positioning issues
        // and because code blocks are incomplete
        if self.is_streaming {
            return;
        }

        let blocks = self.collect_code_blocks();

        for (logical_line, content) in blocks {
            let visual_line = self.wrap_info.logical_to_visual(logical_line);

            // Check visibility
            if visual_line < visual_scroll || visual_line >= visual_scroll + area.height as usize {
                continue;
            }
            let relative_line = visual_line - visual_scroll;

            let header_width = self
                .all_lines
                .get(logical_line)
                .map_or(0, line_display_width);

            if let Some(overlay) = CodeBlockOverlay::new(relative_line, content, area, header_width)
            {
                overlay.render(frame);
                self.code_block_overlay_manager.push(overlay);
            }
        }
    }

    /// Collect all code blocks from messages
    fn collect_code_blocks(&self) -> Vec<(usize, String)> {
        let mut blocks = Vec::new();

        // Pre-calculate message offsets to avoid O(n²) computation.
        // Matches rebuild_flat_lines: every message has a trailing separator line.
        let mut msg_offsets: Vec<usize> = Vec::with_capacity(self.messages.len() + 1);
        msg_offsets.push(0);
        for i in 0..self.messages.len() {
            let prev_offset = msg_offsets[i];
            let cache_len = self.msg_cache.get(i).map_or(0, |c| c.len());
            msg_offsets.push(prev_offset + cache_len + 1); // +1 trailing separator
        }

        // Check streaming content (offset at messages.len())
        if !self.streaming_content.is_empty() {
            let offset = msg_offsets[self.messages.len()];
            for block in self.md_renderer.code_blocks() {
                blocks.push((offset + block.start_line, block.content.clone()));
            }
        }

        // Check historical messages
        for (i, msg) in self.messages.iter().enumerate() {
            if let HistoryMessage::Assistant { content, .. } = msg {
                if !content.is_empty() {
                    let cache = &self.msg_cache[i];
                    let offset = msg_offsets[i];
                    for (j, content) in scan_code_blocks(cache) {
                        blocks.push((offset + j, content));
                    }
                }
            }
        }

        blocks
    }
}

impl Component for ChatView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let visible_height = area.height as usize;
        let width = area.width as usize;
        self.last_visible_height = visible_height;
        self.current_area = Some(area);
        self.scroll_button_area = None;

        // Detect width change and rebuild msg_cache if needed
        if width != self.last_render_width {
            self.last_render_width = width;
            self.rebuild_msg_cache();
        }

        // Snapshot prev total for scroll-stability when streaming while scrolled up
        let prev_total_lines = self.wrap_info.total_lines();

        // Check if historical messages changed BEFORE clearing the flag
        let msg_changed = self.msg_cache_dirty;

        // 1. Rebuild flat_lines if any historical message changed
        if msg_changed {
            self.rebuild_flat_lines();
        }

        // 2. Rebuild all_lines
        //    - msg changed: flat_lines prefix may differ in content, rebuild from scratch
        //    - only streaming changed: suffix-only update
        if msg_changed {
            self.all_lines.clear();
            self.all_lines.extend(self.flat_lines.iter().cloned());
        } else {
            let flat_len = self.flat_lines.len();
            if self.all_lines.len() > flat_len {
                self.all_lines.truncate(flat_len);
            }
            if self.all_lines.len() < flat_len {
                self.all_lines
                    .extend(self.flat_lines[self.all_lines.len()..].iter().cloned());
            }
        }
        let has_streaming = self.is_streaming
            || !self.streaming_content.is_empty()
            || !self.streaming_thinking.is_empty();
        if has_streaming {
            let streaming_lines = self.render_streaming(width);
            self.all_lines.extend(streaming_lines);
        }
        if !self.pending_items.is_empty() {
            self.all_lines
                .extend(render_pending_items(&self.pending_items));
        }

        // 3. Rebuild wrap cache
        //    - msg changed: full rebuild (prefix_len = 0)
        //    - only streaming changed: reuse flat_lines as prefix
        let prefix_len = if msg_changed {
            0
        } else {
            self.flat_lines.len()
        };
        self.wrap_info.rebuild(&self.all_lines, width, prefix_len);

        // 4. Scroll calculation
        let total_visual = self.wrap_info.total_lines();

        // If user has scrolled up, adjust scroll_offset to keep absolute view stable
        // when streaming adds new visual lines below.
        if self.is_scrolled_up() && prev_total_lines > 0 {
            let lines_delta = total_visual as i64 - prev_total_lines as i64;
            self.scroll_offset = (self.scroll_offset as i64 + lines_delta).max(0) as usize;
        }

        let max_scroll = total_visual.saturating_sub(visible_height);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
        let visual_scroll = self.visual_scroll_top();

        // 5. Render: zero-copy borrow of all_lines + wrap_info
        let highlight_style = Style::default()
            .fg(colors::text_primary())
            .bg(colors::selected_bg());

        let global_sel = self.selection.map(|s| {
            let norm = s.normalized();
            (
                (norm.start_line, norm.start_col),
                (norm.end_line, norm.end_col),
            )
        });

        let paragraph = WrapParagraph::new(&self.all_lines, &self.wrap_info)
            .scroll_y(visual_scroll)
            .selection(global_sel)
            .highlight_style(highlight_style);

        frame.render_widget(paragraph, area);

        // Render copy buttons for visible code blocks
        self.render_code_block_buttons(frame, area, visual_scroll);

        // Draw scroll-to-bottom button if user has scrolled up
        if self.is_scrolled_up() {
            self.draw_scroll_button(frame, area);
        }

        // Render context menu if open
        if let Some(ref menu) = self.context_menu {
            menu.render(frame);
        }
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Custom(attr::SCROLL_PROGRESS) => {
                let (current, total) = self.get_scroll_progress();
                // Third value indicates if user has scrolled up from bottom (1 = scrolled, 0 = at bottom)
                let is_scrolled = i32::from(self.is_scrolled_up());
                Some(QueryResult::Owned(AttrValue::String(format!(
                    "{current}\x00{total}\x00{is_scrolled}"
                ))))
            }
            Attribute::Custom(attr::IS_EMPTY) => {
                let empty =
                    self.messages.is_empty() && !self.is_streaming && self.pending_items.is_empty();
                Some(QueryResult::Owned(AttrValue::Flag(empty)))
            }
            _ => self
                .props
                .get(attr)
                .map(|v| QueryResult::Borrowed(v.into())),
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        // Extract the custom string first, then match on it
        let Attribute::Custom(cmd) = attr else {
            self.props.set(attr, value);
            return;
        };

        match cmd {
            attr::ADD_USER_MESSAGE => {
                if let AttrValue::String(blocks_json) = value {
                    // Try parsing as JSON first (for ContentBlock array)
                    if let Ok(content_blocks) =
                        serde_json::from_str::<Vec<ContentBlock>>(&blocks_json)
                    {
                        self.add_user_message(content_blocks);
                    } else {
                        // Fallback to plain text
                        self.add_user_message(vec![ContentBlock::Text { text: blocks_json }]);
                    }
                }
            }
            attr::ADD_STEER_MESSAGE => {
                if let AttrValue::String(blocks_json) = value {
                    if let Ok(content_blocks) =
                        serde_json::from_str::<Vec<ContentBlock>>(&blocks_json)
                    {
                        self.add_steer_message(content_blocks);
                    } else {
                        self.add_steer_message(vec![ContentBlock::Text { text: blocks_json }]);
                    }
                }
            }
            attr::ADD_ERROR_MESSAGE => {
                if let AttrValue::String(error) = value {
                    self.add_error_message(error);
                }
            }
            attr::ADD_NOTICE => {
                if let AttrValue::String(text) = value {
                    self.add_notice(text);
                }
            }
            attr::ADD_ASSISTANT_MSG => {
                if let AttrValue::String(combined) = value {
                    let parts: Vec<&str> = combined.split('\x00').collect();
                    let content = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let thinking = parts
                        .get(1)
                        .filter(|s| !s.is_empty())
                        .map(|s| (*s).to_string());
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok());
                    self.add_assistant_message(content, thinking, elapsed_ms);
                }
            }
            attr::START_STREAMING => self.start_streaming(),
            attr::STOP_STREAMING => self.stop_streaming(),
            attr::CANCEL_STREAMING => self.cancel_streaming(),
            attr::APPEND_CONTENT => {
                if let AttrValue::String(text) = value {
                    self.append_streaming_content(&text);
                }
            }
            attr::APPEND_THINKING => {
                if let AttrValue::String(text) = value {
                    self.append_streaming_thinking(&text);
                }
            }
            attr::SCROLL_UP => self.scroll_up(Self::MOUSE_SCROLL_LINES),
            attr::SCROLL_DOWN => self.scroll_down(Self::MOUSE_SCROLL_LINES),
            attr::SCROLL_TO_BOTTOM => self.scroll_to_bottom(),
            attr::SCROLL_TO_TOP => self.scroll_to_top(),
            attr::TOGGLE_THINKING => self.toggle_last_thinking(),
            attr::TOGGLE_EXPAND_ALL => self.toggle_expand_all(),
            attr::EXPAND_ALL => self.expand_all(),
            attr::COLLAPSE_ALL => self.collapse_all(),
            attr::START_TOOL => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let tool_name = parts
                        .get(1)
                        .map_or_else(|| "tool".to_string(), |s| (*s).to_string());
                    let arguments = parts.get(2).map(|s| (*s).to_string());
                    self.start_tool(tool_id, tool_name, arguments);
                }
            }
            attr::COMPLETE_TOOL => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let output = parts.get(1).map_or(String::new(), |s| (*s).to_string());
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    // Parse content blocks from 4th part (JSON)
                    let content_blocks: Vec<ToolOutputBlock> = parts
                        .get(3)
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();
                    self.complete_tool(&tool_id, output, elapsed_ms, content_blocks);
                }
            }
            attr::FAIL_TOOL => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let error = parts.get(1).map_or(String::new(), |s| (*s).to_string());
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    self.fail_tool(&tool_id, error, elapsed_ms);
                }
            }
            attr::PAGE_UP | attr::PAGE_DOWN => {
                if let AttrValue::Number(height) = value {
                    match cmd {
                        attr::PAGE_UP => self.scroll_up(height as usize),
                        attr::PAGE_DOWN => self.scroll_down(height as usize),
                        _ => {}
                    }
                }
            }
            attr::CLEAR_HISTORY => {
                self.clear_all_caches();
                self.messages.clear();
                self.scroll_offset = 0;
                self.pending_items.clear();
            }
            attr::SET_PENDING_ITEMS => {
                if let AttrValue::String(items_json) = value {
                    if let Ok(items) =
                        serde_json::from_str::<Vec<kernel::comms::MailboxItem>>(&items_json)
                    {
                        self.set_pending_items(items);
                    }
                }
            }
            attr::INIT_SUBAGENT => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let parent_tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let session_id = parts.get(1).map_or(String::new(), |s| (*s).to_string());
                    let description = parts.get(2).map_or(String::new(), |s| (*s).to_string());
                    self.init_subagent(&parent_tool_id, session_id, description);
                }
            }
            attr::UPDATE_SUBAGENT => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let parent_tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    if let Some(json) = parts.get(1) {
                        if let Ok(event) = serde_json::from_str::<kernel::event::Event>(json) {
                            self.update_subagent(&parent_tool_id, event);
                        }
                    }
                }
            }
            attr::FINALIZE_SUBAGENT => {
                if let AttrValue::String(parent_tool_id) = value {
                    self.finalize_subagent(&parent_tool_id);
                }
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::None
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(tuirealm::command::Direction::Up) => {
                self.scroll_up(1);
                CmdResult::NoChange
            }
            Cmd::Move(tuirealm::command::Direction::Down) => {
                self.scroll_down(1);
                CmdResult::NoChange
            }
            _ => CmdResult::NoChange,
        }
    }
}

/// Component wrapper
pub struct ChatViewComponent {
    component: ChatView,
}

impl Default for ChatViewComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatViewComponent {
    pub fn new() -> Self {
        Self {
            component: ChatView::new(),
        }
    }

    /// Initialize history from `SessionMessage` (unified rendering path)
    pub fn init_history(&mut self, messages: &[kernel::types::SessionMessage]) {
        for msg in messages {
            self.add_message_to_history(msg);
        }
    }

    /// Helper to add a single message to history
    fn add_message_to_history(&mut self, msg: &kernel::types::SessionMessage) {
        use kernel::types::SessionMessage;
        match msg {
            SessionMessage::User(user_msg) => {
                if !user_msg.content.is_empty() {
                    self.component.add_user_message(user_msg.content.clone());
                }
            }
            SessionMessage::Steer(steer_msg) => {
                if !steer_msg.content.is_empty() {
                    self.component.add_steer_message(steer_msg.content.clone());
                }
            }
            SessionMessage::Interrupted(marker_msg) => {
                // 中断标记在 TUI 历史里呈现为一条 notice 行（分割线语义）
                let text = marker_msg.text_content();
                if !text.is_empty() {
                    self.component.add_notice(text);
                }
            }
            SessionMessage::Assistant(assistant_msg) => {
                let content = assistant_msg.text_content();
                let thinking = assistant_msg.thinking_content();
                self.component
                    .add_assistant_message(content, thinking, None);
            }
            SessionMessage::Tool(tool_msg) => {
                let output = tool_msg.text_content();
                // Tool messages are self-contained (name + args + result), so
                // history entries are built directly from them. Content blocks
                // are not available during history init; elapsed is unknown.
                self.component.start_tool(
                    tool_msg.tool_call_id.clone(),
                    tool_msg.name.clone(),
                    Some(tool_msg.args.clone()),
                );
                self.component
                    .complete_tool(&tool_msg.tool_call_id, output, 0, Vec::new());
            }
        }
    }
}

impl Component for ChatViewComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.component.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.component.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        use tuirealm::props::PropPayload;
        match attr {
            Attribute::Custom(attr::INIT_HISTORY) => {
                if let AttrValue::Payload(PropPayload::Any(payload)) = value {
                    let any = payload.as_any();
                    if let Some(messages) = any.downcast_ref::<Vec<kernel::types::SessionMessage>>()
                    {
                        self.init_history(messages);
                    }
                }
            }
            _ => self.component.attr(attr, value),
        }
    }

    fn state(&self) -> State {
        self.component.state()
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.component.perform(cmd)
    }
}

impl AppComponent<Msg, crate::msg::UserEvent> for ChatViewComponent {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        use tuirealm::event::MouseEvent;
        use tuirealm::event::MouseEventKind;

        match *ev {
            Event::Tick => {
                // Chat view has no per-frame animations; tick() never
                // requests a redraw. Kept as a hook for future animations.
                if self.component.tick() {
                    Some(Msg::Redraw)
                } else {
                    None
                }
            }
            // Keyboard scrolling - PageUp/PageDown
            Event::Keyboard(KeyEvent {
                code: Key::PageUp,
                modifiers: KeyModifiers::NONE,
            }) => {
                let amount = self.component.last_visible_height.max(1);
                self.component.scroll_up(amount);
                Some(Msg::Redraw)
            }
            Event::Keyboard(KeyEvent {
                code: Key::PageDown,
                modifiers: KeyModifiers::NONE,
            }) => {
                let amount = self.component.last_visible_height.max(1);
                self.component.scroll_down(amount);
                Some(Msg::Redraw)
            }
            // Handle mouse scroll events for chat view scrolling
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                ..
            }) => {
                self.component.scroll_up(ChatView::MOUSE_SCROLL_LINES);
                Some(Msg::Redraw)
            }
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..
            }) => {
                self.component.scroll_down(ChatView::MOUSE_SCROLL_LINES);
                Some(Msg::Redraw)
            }
            // Handle mouse events for text selection and scroll button
            Event::Mouse(MouseEvent {
                kind, column, row, ..
            }) => {
                let action = self.component.handle_mouse_event(kind, column, row);
                match action {
                    MouseAction::ScrollToBottom => Some(Msg::Redraw),
                    MouseAction::Copied(text) => {
                        // Show status message with copied text preview (limit display width)
                        let msg = format!("📋 {text}");
                        Some(Msg::Notification(Notification::unknown(msg, 2000)))
                    }
                    MouseAction::CodeCopied => Some(Msg::Notification(Notification::unknown(
                        "📋 Code copied".to_string(),
                        1500,
                    ))),
                    MouseAction::MenuAction(msg) => {
                        Some(Msg::Notification(Notification::unknown(msg, 1500)))
                    }
                    MouseAction::None => {
                        if matches!(kind, MouseEventKind::Down(_) | MouseEventKind::Drag(_)) {
                            // Selection in progress or context menu opened, just redraw
                            Some(Msg::Redraw)
                        } else {
                            None
                        }
                    }
                }
            }
            _ => None,
        }
    }
}

/// Word characters for double-click selection. Beyond alphanumeric and `_`,
/// include characters common in paths and URLs so a double-click selects a
/// whole path/URL instead of a fragment.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '_' | '-' | '.' | '/' | ':' | '~' | '@' | '%' | '+' | '=' | '?' | '&' | '#'
        )
}

/// Convert a terminal column position to a character index within a text segment.
/// Used for mouse click coordinate conversion.
fn display_col_to_char_idx(text: &str, start_byte: usize, end_byte: usize, col: usize) -> usize {
    let segment = &text[start_byte..end_byte];
    let mut current_width = 0;
    let mut char_count = 0;

    for (idx, ch) in segment.chars().enumerate() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + ch_width > col {
            return char_count;
        }
        current_width += ch_width;
        char_count = idx + 1;
    }

    char_count
}

#[cfg(test)]
#[path = "core_test.rs"]
mod tests;
