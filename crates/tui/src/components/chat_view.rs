//! Unified chat view component
//!
//! Displays chat history + streaming message in a single scrollable view.

use std::sync::Arc;

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::Rect,
        style::{Modifier, Style},
        text::{Line, Span, Text},
        Frame,
    },
    state::{State, StateValue},
};

use super::wrap_paragraph::WrapParagraph;

use crate::{
    attr,
    components::info_bar::Notification,
    markdown_stream::StreamingMarkdownRenderer,
    msg::Msg,
    theme::{chars, colors},
    utils::text::{char_idx_to_byte_idx, preprocess, substring_by_chars, truncate_by_chars, truncate_by_width},
};
use kernel::tools::{
    EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, READ_TOOL_NAME, REMINDER_TOOL_NAME,
    SHELL_TOOL_NAME, SKILL_FILENAME, SKILL_TOOL_NAME, SUBAGENT_TOOL_NAME, TODO_READ_TOOL_NAME,
    WEBFETCH_TOOL_NAME, WEBSEARCH_TOOL_NAME, WRITE_TOOL_NAME,
};
use kernel::types::{ContentBlock, ToolOutputBlock};
use kernel::utils::tokens;
use kernel::{
    task::{TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME, TASK_UPDATE_TOOL_NAME},
    tools::TODO_WRITE_TOOL_NAME,
};

use super::banner::MascotAnimator;

/// Tool execution status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Result of handling a mouse event
#[derive(Debug)]
pub enum MouseAction {
    /// Selection was copied to clipboard
    Copied(String),
    /// Scroll-to-bottom button was clicked
    ScrollToBottom,
    /// No action taken
    None,
}

/// A chat message in history
#[derive(Debug, Clone)]
pub enum HistoryMessage {
    User(Vec<ContentBlock>),
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
        tokens: Option<u32>,
        progress: Option<String>,
        content_blocks: Vec<ToolOutputBlock>,
    },
    Error(String),
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
    tick_frame: usize,
    md_renderer: StreamingMarkdownRenderer,

    // Track active tool executions
    active_tools: std::collections::HashMap<String, (String, ToolStatus)>, // tool_id -> (tool_name, status)
    // Expand all mode (ctrl-o): show all thinking and tool details
    expand_all: bool,
    // Banner data (rendered as first content, scrolls with messages)
    banner: Option<crate::components::BannerData>,
    // Queued message waiting to be sent (displayed at bottom during streaming)
    queued_message: Option<Vec<ContentBlock>>,
    // Mascot animator for blinking animation
    mascot_animator: MascotAnimator,
    // Track visible height for scroll calculations
    last_visible_height: usize,
    // Message-level cache: None means dirty (needs re-render), Some means cached (Arc for sharing)
    msg_cache: Vec<Option<Vec<Arc<Line<'static>>>>>,
    // Banner cache (separate because mascot animates)
    banner_cache: Vec<Arc<Line<'static>>>,
    banner_dirty: bool,
    // Message lines cache (excluding banner and streaming)
    msg_lines: Vec<Arc<Line<'static>>>,
    msg_cache_dirty: bool,
    // Viewport cache: only the currently visible lines (cloned from Arc)
    viewport_lines: Vec<Line<'static>>,
    last_viewport: (usize, usize), // (start_line, end_line)
    // First visible row within the first viewport line (for accurate mouse coord conversion)
    viewport_first_row_offset: usize,
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
    // Cached total visual line count (updated in view)
    total_visual_lines: usize,
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
            tick_frame: 0,
            md_renderer: StreamingMarkdownRenderer::new(),

            active_tools: std::collections::HashMap::new(),
            expand_all: false,
            banner: Some(crate::components::BannerData::default()),
            queued_message: None,
            mascot_animator: MascotAnimator::default(),
            last_visible_height: 0,
            msg_cache: Vec::new(),
            banner_cache: Vec::new(),
            banner_dirty: true,
            msg_lines: Vec::new(),
            msg_cache_dirty: true,
            viewport_lines: Vec::new(),
            last_viewport: (0, 0),
            viewport_first_row_offset: 0,
            selection: None,
            is_selecting: false,
            last_click_time: None,
            last_click_pos: None,
            current_area: None,
            scroll_button_area: None,
            total_visual_lines: 0,
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
            self.msg_cache[idx] = None;
            self.msg_cache_dirty = true;
        }
    }

    /// Add a new empty cache entry for a new message.
    fn push_new_msg_cache(&mut self) {
        self.msg_cache.push(None);
        self.msg_cache_dirty = true;
    }

    /// Invalidate all message caches.
    fn invalidate_all_caches(&mut self) {
        for cache in &mut self.msg_cache {
            *cache = None;
        }
        self.msg_lines.clear();
        self.msg_cache_dirty = true;
    }

    /// Clear all caches and messages.
    fn clear_all_caches(&mut self) {
        self.msg_cache.clear();
        self.msg_lines.clear();
        self.viewport_lines.clear();
        self.banner_cache.clear();
        self.last_viewport = (0, 0);
        self.viewport_first_row_offset = 0;
        self.msg_cache_dirty = true;
        self.banner_dirty = true;
        self.selection = None;
        self.is_selecting = false;
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

    /// Set banner data to display at the top
    pub fn set_banner(&mut self, banner: crate::components::BannerData) {
        self.banner = Some(banner);
        self.banner_dirty = true;
    }

    /// Set queued message to display during streaming
    pub fn set_queued_message(&mut self, blocks: Vec<ContentBlock>) {
        self.queued_message = Some(blocks);
        self.msg_cache_dirty = true;
    }

    /// Clear the queued message
    pub fn clear_queued_message(&mut self) {
        self.queued_message = None;
        self.msg_cache_dirty = true;
    }

    pub fn add_user_message(&mut self, content_blocks: Vec<ContentBlock>) {
        self.messages.push(HistoryMessage::User(content_blocks));
        self.push_new_msg_cache();
    }

    pub fn add_error_message(&mut self, error: String) {
        self.messages.push(HistoryMessage::Error(error));
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

        self.active_tools
            .insert(tool_id.clone(), (tool_name.clone(), ToolStatus::Running));
        self.messages.push(HistoryMessage::Tool {
            tool_name,
            tool_id,
            status: ToolStatus::Running,
            output: None,
            error: None,
            folded: !self.expand_all,
            arguments,
            elapsed_ms: None,
            tokens: None,
            progress: None,
            content_blocks: Vec::new(),
        });
        self.push_new_msg_cache();
    }

    pub fn complete_tool(
        &mut self,
        tool_id: String,
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
                if id == &tool_id {
                    *status = ToolStatus::Completed;
                    *out = Some(output);
                    *elapsed = Some(elapsed_ms);
                    *blocks = content_blocks;
                    self.invalidate_msg_cache(i);
                    break;
                }
            }
        }
        // Update active tools tracking
        if let Some((name, _)) = self.active_tools.remove(&tool_id) {
            self.active_tools
                .insert(tool_id, (name, ToolStatus::Completed));
        }
    }

    pub fn fail_tool(&mut self, tool_id: String, error: String, elapsed_ms: u64) {
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
                if id == &tool_id {
                    *status = ToolStatus::Failed;
                    *err = Some(error);
                    *elapsed = Some(elapsed_ms);
                    self.invalidate_msg_cache(i);
                    break;
                }
            }
        }
        // Update active tools tracking
        if let Some((name, _)) = self.active_tools.remove(&tool_id) {
            self.active_tools
                .insert(tool_id, (name, ToolStatus::Failed));
        }
    }

    /// Update tool progress (for long-running tools like subagent)
    pub fn update_tool_progress(&mut self, tool_id: &str, message: &str, tokens: Option<u32>) {
        for (i, msg) in self.messages.iter_mut().enumerate().rev() {
            if let HistoryMessage::Tool {
                tool_id: id,
                progress,
                tokens: tok,
                ..
            } = msg
            {
                if id == tool_id {
                    *progress = Some(message.to_string());
                    *tok = tokens;
                    self.invalidate_msg_cache(i);
                    break;
                }
            }
        }
    }

    /// Flush pending streaming content to history
    /// Called when a new block starts (tool, code block, etc.) to preserve current content
    pub fn flush_streaming(&mut self) {
        // If there's pending thinking content, save it as an assistant message
        if !self.streaming_thinking.is_empty() {
            self.messages.push(HistoryMessage::Assistant {
                content: String::new(),
                thinking: Some(self.streaming_thinking.clone()),
                thinking_folded: !self.expand_all,
                thinking_elapsed_ms: None,
            });
            self.push_new_msg_cache();
            self.streaming_thinking.clear();
        }

        // If there's pending content, save it as an assistant message
        if !self.streaming_content.is_empty() {
            self.messages.push(HistoryMessage::Assistant {
                content: self.streaming_content.clone(),
                thinking: None,
                thinking_folded: true,
                thinking_elapsed_ms: None,
            });
            self.push_new_msg_cache();
            self.streaming_content.clear();
            self.md_renderer = StreamingMarkdownRenderer::new();
        }
    }

    pub fn start_streaming(&mut self) {
        self.is_streaming = true;
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.tick_frame = 0;
        // Note: Don't reset scroll_offset here - respect user's scroll position
    }

    pub fn stop_streaming(&mut self) {
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.is_streaming = false;
        // Streaming content affects rendered output
        self.msg_cache_dirty = true;
    }

    /// Cancel streaming - flush partial content and mark running tools as cancelled
    pub fn cancel_streaming(&mut self) {
        // Note: Content is already saved by app.rs via add_assistant_with_thinking
        // Just clear streaming buffers without flushing to avoid duplicates
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.is_streaming = false;
        // Mark any running tools as cancelled and invalidate their caches
        let mut indices_to_invalidate = Vec::new();
        for (tool_id, (_, status)) in &mut self.active_tools {
            if *status == ToolStatus::Running {
                *status = ToolStatus::Cancelled;
                for (i, msg) in self.messages.iter().enumerate().rev() {
                    if let HistoryMessage::Tool { tool_id: id, .. } = msg {
                        if id == tool_id {
                            indices_to_invalidate.push(i);
                            break;
                        }
                    }
                }
            }
        }
        // Update message statuses and invalidate caches
        for idx in indices_to_invalidate {
            if let Some(HistoryMessage::Tool { status, .. }) = self.messages.get_mut(idx) {
                *status = ToolStatus::Cancelled;
            }
            self.invalidate_msg_cache(idx);
        }
    }

    pub fn append_streaming_content(&mut self, text: &str) {
        self.streaming_content.push_str(text);
        self.md_renderer.append(text);
        // Streaming content affects rendered output
        self.msg_cache_dirty = true;
    }

    pub fn append_streaming_thinking(&mut self, text: &str) {
        self.streaming_thinking.push_str(text);
        // Streaming content affects rendered output
        self.msg_cache_dirty = true;
    }

    pub fn tick(&mut self) {
        if self.is_streaming {
            self.tick_frame = self.tick_frame.wrapping_add(1);
        }
        // Update mascot blink animation
        if self.mascot_animator.tick() {
            self.banner_dirty = true;
        }
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
        self.scroll_offset = self.total_visual_lines.saturating_sub(visible);
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

    /// Get scroll progress for browse mode (`current_line`, `total_lines`)
    /// Returns the 1-based current visible start position and total lines
    /// Note: now uses visual lines (post-wrap) instead of logical lines
    pub fn get_scroll_progress(&self) -> (usize, usize) {
        if self.total_visual_lines == 0 {
            return (0, 0);
        }

        // Calculate current visible start position (1-based)
        // scroll_offset = 0 means at bottom showing latest content
        // scroll_offset > 0 means scrolled up by that many visual lines from bottom
        let start_visual_line = if self.scroll_offset == 0 {
            // At bottom: show the last visible_height lines
            self.total_visual_lines
                .saturating_sub(self.last_visible_height.saturating_sub(1))
                .max(1)
        } else {
            // Scrolled up: start_line is scroll_offset lines from bottom
            self.total_visual_lines
                .saturating_sub(self.scroll_offset)
                .max(1)
        };

        (
            start_visual_line.min(self.total_visual_lines),
            self.total_visual_lines,
        )
    }

    pub fn toggle_expand_all(&mut self) {
        self.expand_all = !self.expand_all;
        // Update all messages to reflect expand_all state
        for msg in &mut self.messages {
            match msg {
                HistoryMessage::Assistant {
                    thinking_folded, ..
                } => {
                    *thinking_folded = !self.expand_all;
                }
                HistoryMessage::Tool { folded, .. } => {
                    *folded = !self.expand_all;
                }
                _ => {}
            }
        }
        self.invalidate_all_caches();
    }

    pub fn expand_all(&mut self) {
        if !self.expand_all {
            self.expand_all = true;
            for msg in &mut self.messages {
                match msg {
                    HistoryMessage::Assistant {
                        thinking_folded, ..
                    } => {
                        *thinking_folded = false;
                    }
                    HistoryMessage::Tool { folded, .. } => {
                        *folded = false;
                    }
                    _ => {}
                }
            }
            self.invalidate_all_caches();
        }
    }

    pub fn collapse_all(&mut self) {
        if self.expand_all {
            self.expand_all = false;
            for msg in &mut self.messages {
                match msg {
                    HistoryMessage::Assistant {
                        thinking_folded, ..
                    } => {
                        *thinking_folded = true;
                    }
                    HistoryMessage::Tool { folded, .. } => {
                        *folded = true;
                    }
                    _ => {}
                }
            }
            self.invalidate_all_caches();
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn render_message(msg: &HistoryMessage, width: usize) -> Vec<Arc<Line<'static>>> {
        let mut lines = Vec::new();

        match msg {
            HistoryMessage::User(content_blocks) => {
                let user_bg = colors::user_msg_bg();
                let mut line_idx = 0;
                for block in content_blocks {
                    match block {
                        ContentBlock::Text { text } => {
                            for line in text.lines() {
                                let prefix = if line_idx == 0 {
                                    chars::INPUT_PROMPT
                                } else {
                                    chars::INPUT_PROMPT_MULTI
                                };
                                lines.push(Arc::new(Line::from(vec![
                                    Span::styled(
                                        prefix,
                                        Style::default()
                                            .fg(colors::accent_user())
                                            .bg(user_bg)
                                            .add_modifier(Modifier::BOLD),
                                    ),
                                    Span::styled(
                                        preprocess(line),
                                        Style::default().fg(colors::text_primary()).bg(user_bg),
                                    ),
                                ])));
                                line_idx += 1;
                            }
                        }
                        ContentBlock::ImageUrl { .. } => {
                            let prefix = if line_idx == 0 {
                                chars::INPUT_PROMPT
                            } else {
                                chars::INPUT_PROMPT_MULTI
                            };
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(
                                    prefix,
                                    Style::default()
                                        .fg(colors::accent_user())
                                        .bg(user_bg)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(
                                    "[Image]",
                                    Style::default().fg(colors::text_secondary()).bg(user_bg),
                                ),
                            ])));
                            line_idx += 1;
                        }
                        _ => {}
                    }
                }
            }
            HistoryMessage::Assistant {
                content,
                thinking,
                thinking_folded,
                thinking_elapsed_ms,
            } => {
                // Render thinking summary (folded) or detail (expanded)
                let thinking_rendered = thinking.as_ref().is_some_and(|t| {
                    Self::render_thinking_lines(
                        &mut lines,
                        t,
                        *thinking_folded,
                        *thinking_elapsed_ms,
                    )
                });

                // Add separator between thinking and content if both exist
                if thinking_rendered && !content.is_empty() {
                    lines.push(Arc::new(Line::from("")));
                }

                // Render content with markdown (no indicator)
                // Note: no empty line here, thinking already adds one if present
                if !content.is_empty() {
                    let mut md_renderer = StreamingMarkdownRenderer::new();
                    md_renderer.set_content(content.clone());
                    let md_lines = md_renderer.lines();

                    for line in md_lines {
                        lines.push(Arc::new(line.clone()));
                    }
                }
            }
            HistoryMessage::Tool {
                tool_name,
                tool_id: _,
                status,
                output,
                error,
                folded,
                arguments,
                elapsed_ms,
                ref tokens,
                ref progress,
                content_blocks,
            } => {
                let color = match status {
                    ToolStatus::Running => colors::accent_warning(),
                    ToolStatus::Completed => colors::accent_success(),
                    ToolStatus::Failed => colors::accent_error(),
                    ToolStatus::Cancelled => colors::text_secondary(),
                };
                let icon = tool_icon(tool_name);

                // Build header with execution time (only show if >= 1s)
                let time_str = elapsed_ms
                    .filter(|ms| *ms >= 1000)
                    .map(|ms| format!(" {:.1}s", ms as f64 / 1000.0))
                    .unwrap_or_default();

                // Peek args in folded mode (max 150 chars, compact whitespace)
                let peek_args = if *folded {
                    arguments.as_ref().and_then(|args| {
                        let compact = sanitize_single_line(args);
                        if compact.is_empty() {
                            None
                        } else {
                            let peek = truncate_by_chars(&compact, 150);
                            Some(peek)
                        }
                    })
                } else {
                    None
                };

                // Build header line with tool name and target (e.g. "Read src/main.rs")
                let tool_name_display = to_camel_case(tool_name);
                let target = extract_tool_target(tool_name, arguments.as_deref());

                // Tool name with status color
                let tool_part = format!("{icon}{tool_name_display}{time_str}");
                let mut header_spans = vec![Span::styled(
                    tool_part,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )];

                // Target/args with text_primary color (no bold)
                if let Some(t) = target {
                    header_spans.push(Span::styled(
                        format!(" {t}"),
                        Style::default().fg(colors::text_primary()),
                    ));
                } else if let Some(peek) = peek_args {
                    // Fallback to peek_args if we couldn't extract a target
                    header_spans.push(Span::styled(
                        format!(" {peek}"),
                        Style::default().fg(colors::text_primary()),
                    ));
                }

                // For bash commands, add timeout/async info with text_secondary style
                if tool_name == SHELL_TOOL_NAME {
                    if let Some(ref args) = arguments {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(args) {
                            let timeout_secs = value["timeout"].as_u64();
                            let background = value["background"].as_bool().unwrap_or(false);

                            // Show async badge when background mode is enabled
                            if background {
                                header_spans.push(Span::styled(
                                    " async".to_string(),
                                    Style::default().fg(colors::text_secondary()),
                                ));
                            }

                            // Show timeout if explicitly set (or non-default for sync mode)
                            if let Some(t) = timeout_secs {
                                if background || t != 60 {
                                    header_spans.push(Span::styled(
                                        format!(" timeout {t}s"),
                                        Style::default().fg(colors::text_secondary()),
                                    ));
                                }
                            }
                        }
                    }
                }

                lines.push(Arc::new(Line::from(header_spans)));

                // Output peek in folded mode (max 50 chars, indented)
                if *folded {
                    // Show progress for running tools
                    if *status == ToolStatus::Running {
                        if let Some(ref prog) = progress {
                            let prog_text = sanitize_single_line(prog);
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(" ⎿ ", Style::default().fg(colors::text_secondary())),
                                Span::styled(
                                    prog_text,
                                    Style::default().fg(colors::text_secondary()),
                                ),
                            ])));
                        }
                    }

                    // Show tokens if available
                    if let Some(total) = tokens {
                        let token_text = format!(" ⎿ {} tokens", tokens::format_tokens(*total));
                        lines.push(Arc::new(Line::from(vec![Span::styled(
                            token_text,
                            Style::default().fg(colors::text_secondary()),
                        )])));
                    }

                    // Show output peek in folded mode (max 2 lines based on width)
                    if let Some(out) = error.as_ref().or(output.as_ref()) {
                        let trimmed = out.trim();
                        if !trimmed.is_empty() {
                            // Compact whitespace first, then truncate to 2 lines width
                            let compact = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
                            // Total width for 2 lines, minus the " ⎿ " prefix
                            let max_width = width * 2 - 3;
                            let peek = truncate_by_width(&compact, max_width, "...");
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(" ⎿ ", Style::default().fg(colors::text_secondary())),
                                Span::styled(peek, Style::default().fg(colors::text_secondary())),
                            ])));
                        }
                    }
                }

                if !*folded {
                    // Show tool arguments if available
                    if let Some(args) = arguments {
                        if !args.is_empty() {
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(
                                    chars::MSG_INDENT_GUIDE,
                                    Style::default().fg(colors::text_secondary()),
                                ),
                                Span::styled(
                                    "Arguments:",
                                    Style::default()
                                        .fg(colors::text_secondary())
                                        .add_modifier(Modifier::BOLD),
                                ),
                            ])));
                            for line in args.lines() {
                                lines.push(Arc::new(Line::from(vec![
                                    Span::styled(
                                        chars::MSG_INDENT2_GUIDE,
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                    Span::styled(
                                        preprocess(line),
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                ])));
                            }
                        }
                    }

                    if let Some(err) = error {
                        for line in err.lines() {
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(
                                    chars::MSG_INDENT_GUIDE,
                                    Style::default().fg(colors::accent_error()),
                                ),
                                Span::styled(
                                    preprocess(line),
                                    Style::default().fg(colors::accent_error()),
                                ),
                            ])));
                        }
                    } else if let Some(out) = output {
                        lines.push(Arc::new(Line::from(vec![
                            Span::styled(
                                chars::MSG_INDENT_GUIDE,
                                Style::default().fg(colors::text_secondary()),
                            ),
                            Span::styled(
                                "Output:",
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])));
                        for line in out.lines() {
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(
                                    chars::MSG_INDENT_GUIDE,
                                    Style::default().fg(colors::accent_system()),
                                ),
                                Span::styled(
                                    preprocess(line),
                                    Style::default().fg(colors::text_primary()),
                                ),
                            ])));
                        }
                    } else if *status == ToolStatus::Running {
                        let running_text = progress.as_ref().map_or_else(
                            || "Running...".to_string(),
                            |p| format!("Running: {}", sanitize_single_line(p)),
                        );
                        lines.push(Arc::new(Line::from(vec![
                            Span::styled(
                                chars::MSG_INDENT_GUIDE,
                                Style::default().fg(colors::text_secondary()),
                            ),
                            Span::styled(
                                running_text,
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ])));
                    } else if *status == ToolStatus::Cancelled {
                        lines.push(Arc::new(Line::from(vec![
                            Span::styled(
                                chars::MSG_INDENT_GUIDE,
                                Style::default().fg(colors::text_secondary()),
                            ),
                            Span::styled(
                                "Cancelled",
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ])));
                    }

                    // Show image details in unfolded mode
                    for block in content_blocks {
                        if let ToolOutputBlock::Image { url, mime_type, .. } = block {
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(
                                    chars::MSG_INDENT_GUIDE,
                                    Style::default().fg(colors::text_secondary()),
                                ),
                                Span::styled(
                                    "Image:",
                                    Style::default()
                                        .fg(colors::text_secondary())
                                        .add_modifier(Modifier::BOLD),
                                ),
                            ])));
                            let url_display = truncate_by_chars(url, 100);
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(
                                    chars::MSG_INDENT2_GUIDE,
                                    Style::default().fg(colors::text_secondary()),
                                ),
                                Span::styled(
                                    url_display,
                                    Style::default().fg(colors::text_primary()),
                                ),
                            ])));
                            if let Some(mime) = mime_type {
                                lines.push(Arc::new(Line::from(vec![
                                    Span::styled(
                                        chars::MSG_INDENT2_GUIDE,
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                    Span::styled(
                                        format!("Type: {mime}"),
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                ])));
                            }
                        }
                    }
                }
            }
            HistoryMessage::Error(error) => {
                // Render error message with red color
                for line in error.lines() {
                    lines.push(Arc::new(Line::from(vec![Span::styled(
                        preprocess(line),
                        Style::default().fg(colors::accent_error()),
                    )])));
                }
            }
        }

        lines
    }

    fn render_streaming(&mut self) -> Vec<Arc<Line<'static>>> {
        let mut lines = Vec::new();

        // Render thinking if present (collapsed by default, expanded in expand_all mode)
        Self::render_thinking_lines(&mut lines, &self.streaming_thinking, !self.expand_all, None);

        // Render content (no indicator, status shown in status bar)
        // Add separator between thinking and content
        if !self.streaming_thinking.is_empty() && !self.streaming_content.is_empty() {
            lines.push(Arc::new(Line::from("")));
        }
        let md_lines = self.md_renderer.lines();

        for line in md_lines {
            lines.push(Arc::new(line.clone()));
        }

        // Add empty line placeholder only if no thinking (thinking already adds one)
        if md_lines.is_empty() && self.streaming_thinking.is_empty() {
            lines.push(Arc::new(Line::from("")));
        }

        lines
    }

    /// Extract text content from content blocks
    fn extract_text_from_blocks(blocks: &[ContentBlock]) -> String {
        blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render queued message to be displayed at the bottom during streaming
    fn render_queued_message(blocks: &[ContentBlock]) -> Vec<Arc<Line<'static>>> {
        let mut lines = Vec::new();

        let text_content = Self::extract_text_from_blocks(blocks);

        // Render header with indicator
        lines.push(Arc::new(Line::from(vec![Span::styled(
            "󰔟 Queued (will send when streaming ends)",
            Style::default()
                .fg(colors::text_secondary())
                .add_modifier(Modifier::ITALIC),
        )])));

        // Render content with dimmed style
        for line in text_content.lines() {
            lines.push(Arc::new(Line::from(vec![
                Span::styled(
                    chars::MSG_INDENT_GUIDE,
                    Style::default().fg(colors::text_secondary()),
                ),
                Span::styled(
                    preprocess(line),
                    Style::default().fg(colors::text_secondary()),
                ),
            ])));
        }

        lines
    }

    /// Render thinking content with optional elapsed time
    ///
    /// Returns true if thinking was rendered (i.e., thinking was non-empty)
    #[allow(clippy::cast_precision_loss)]
    fn render_thinking_lines(
        lines: &mut Vec<Arc<Line<'static>>>,
        thinking: &str,
        is_folded: bool,
        elapsed_ms: Option<u64>,
    ) -> bool {
        if thinking.is_empty() {
            return false;
        }

        let tokens = tokens::estimate_tokens(thinking);
        let elapsed_str = elapsed_ms
            .map(|ms| format!(" · {:.1}s", ms as f64 / 1000.0))
            .unwrap_or_default();

        lines.push(Arc::new(Line::from(vec![Span::styled(
            format!(" Thinking ({tokens} tokens){elapsed_str}"),
            Style::default()
                .fg(colors::text_secondary())
                .add_modifier(Modifier::ITALIC),
        )])));

        if !is_folded {
            for line in thinking.lines() {
                lines.push(Arc::new(Line::from(vec![
                    Span::styled(
                        chars::MSG_INDENT_GUIDE,
                        Style::default().fg(colors::text_secondary()),
                    ),
                    Span::styled(
                        preprocess(line),
                        Style::default().fg(colors::text_secondary()),
                    ),
                ])));
            }
        }

        true
    }
}

impl ChatView {
    const MASCOT_COL_WIDTH: usize = 8;
    const MOUSE_SCROLL_LINES: usize = 2;

    /// Rebuild banner cache (separate because mascot animates).
    fn rebuild_banner_cache(&mut self) {
        if !self.banner_dirty {
            return;
        }
        self.banner_cache.clear();

        if let Some(ref banner) = self.banner {
            let mascot_lines: Vec<&str> = self.mascot_animator.current_lines();
            let info_lines = banner.info_lines();
            let max_rows = mascot_lines.len().max(info_lines.len());

            for i in 0..max_rows {
                let mascot_part = mascot_lines.get(i).unwrap_or(&"");
                let mascot_padded = format!("{mascot_part:width$}", width = Self::MASCOT_COL_WIDTH);
                let mascot_span = Span::styled(mascot_padded, colors::accent_system());

                // info_lines[i] is already a Line with styled spans
                // Clone spans to create 'static lifetime line for cache
                let mut combined_spans: Vec<Span<'static>> = vec![mascot_span];
                if let Some(info_line) = info_lines.get(i) {
                    for span in &info_line.spans {
                        let cloned_span = Span::styled(
                            span.content.to_string(),
                            span.style,
                        );
                        combined_spans.push(cloned_span);
                    }
                }

                self.banner_cache.push(Arc::new(Line::from(combined_spans)));
            }
            self.banner_cache.push(Arc::new(Line::from("")));
        }
        self.banner_dirty = false;
    }

    /// Rebuild `msg_lines` from `msg_cache` if dirty.
    fn rebuild_msg_cache(&mut self) {
        if !self.msg_cache_dirty {
            return;
        }
        self.msg_lines.clear();

        // Get width from current area for calculating output peek (2 lines max)
        let width = self.current_area.map_or(80, |a| a.width as usize);

        for (i, msg) in self.messages.iter().enumerate() {
            let msg_lines = match &self.msg_cache[i] {
                Some(lines) => lines,
                None => {
                    let rendered = Self::render_message(msg, width);
                    self.msg_cache[i] = Some(rendered);
                    self.msg_cache[i].as_ref().unwrap()
                }
            };
            self.msg_lines.extend(msg_lines.iter().cloned());
            if i < self.messages.len() - 1 {
                self.msg_lines.push(Arc::new(Line::from("")));
            }
        }

        self.msg_cache_dirty = false;
    }

    /// Get all lines (banner + messages + streaming) for viewport calculation.
    fn all_lines(&mut self) -> Vec<Arc<Line<'static>>> {
        self.rebuild_banner_cache();
        self.rebuild_msg_cache();

        let mut result = self.banner_cache.clone();
        result.extend(self.msg_lines.iter().cloned());

        // Add streaming content if any
        let has_streaming = self.is_streaming
            || !self.streaming_content.is_empty()
            || !self.streaming_thinking.is_empty();
        if !self.messages.is_empty() && has_streaming {
            result.push(Arc::new(Line::from("")));
        }
        if has_streaming {
            result.extend(self.render_streaming());
        }

        // Add queued message if any (displayed at bottom during streaming)
        if let Some(ref queued) = self.queued_message {
            result.push(Arc::new(Line::from("")));
            result.extend(Self::render_queued_message(queued));
        }

        result
    }

    /// Calculate wrap boundaries using display width (Unicode-aware).
    /// Returns vector of byte indices where each visual row starts.
    fn calculate_wrap_boundaries(text: &str, width: usize) -> Vec<usize> {
        if width == 0 || text.is_empty() {
            return vec![0];
        }

        let mut boundaries = vec![0];
        let mut current_width = 0;
        let mut byte_idx = 0;

        for ch in text.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

            // Check if adding this character would exceed width
            if current_width + ch_width > width && current_width > 0 {
                boundaries.push(byte_idx);
                current_width = ch_width;
            } else {
                current_width += ch_width;
            }
            byte_idx += ch.len_utf8();
        }

        boundaries
    }

    /// Convert display column to character index within a visual row.
    /// `row_start` and `row_end` are byte indices.
    /// Returns character index (0-based).
    fn display_col_to_char_idx(
        text: &str,
        row_start_byte: usize,
        row_end_byte: usize,
        target_col: usize,
    ) -> usize {
        let mut display_col = 0;
        let mut char_idx = 0;

        // Convert byte indices to char positions (handle cases where byte indices
        // might not be at char boundaries by finding the nearest valid positions)
        let safe_start_byte = row_start_byte.min(text.len());
        let safe_end_byte = row_end_byte.min(text.len());

        // Use byte-based slicing carefully to avoid panics
        let start_char_idx = text.get(..safe_start_byte).map_or(0, |s| s.chars().count());
        let end_char_idx = text
            .get(..safe_end_byte)
            .map_or_else(|| text.chars().count(), |s| s.chars().count());

        for (i, ch) in text.chars().enumerate() {
            if i < start_char_idx {
                continue;
            }
            if i >= end_char_idx {
                break;
            }

            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

            // Check if target column is within this character's display range
            if display_col + ch_width > target_col {
                return i;
            }

            display_col += ch_width;
            char_idx = i + 1;
        }

        // Target column is past all characters, return end
        char_idx.min(end_char_idx)
    }
    /// Convert screen coordinates to line/column in visible content.
    /// Uses textwrap to accurately map visual coordinates to character positions.
    fn screen_to_position(
        &self,
        mouse_x: u16,
        mouse_y: u16,
        width: usize,
    ) -> Option<(usize, usize)> {
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

        // Calculate which logical line and visual row within that line
        let viewport_start = self.last_viewport.0;
        // Adjust terminal_row by the first line's offset (since first line may be partially scrolled)
        let adjusted_terminal_row = terminal_row + self.viewport_first_row_offset;
        let mut current_row = 0;

        for (i, line) in self.viewport_lines.iter().enumerate() {
            let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

            // Calculate wrap boundaries for this line
            let boundaries = Self::calculate_wrap_boundaries(&line_text, width);
            let wrapped_height = boundaries.len();

            if current_row + wrapped_height > adjusted_terminal_row {
                // This logical line contains the clicked terminal row
                let line_idx = viewport_start + i;
                let visual_row_in_line = adjusted_terminal_row - current_row;

                // Calculate character column based on visual row
                // boundaries contains byte indices, so these are byte positions
                let row_start_byte = boundaries.get(visual_row_in_line).copied().unwrap_or(0);
                let row_end_byte = boundaries
                    .get(visual_row_in_line + 1)
                    .copied()
                    .unwrap_or(line_text.len());

                // Convert display column to character index within this visual row
                let char_col = Self::display_col_to_char_idx(
                    &line_text,
                    row_start_byte,
                    row_end_byte,
                    terminal_col,
                );

                return Some((line_idx, char_col));
            }

            current_row += wrapped_height;
        }

        // Click is past all visible lines - use the last line
        let last_idx = viewport_start + self.viewport_lines.len().saturating_sub(1);
        self.viewport_lines.last().map(|line| {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            (last_idx, text.chars().count())
        })
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

        let all_lines = self.all_lines_for_selection();
        tracing::debug!("get_selected_text: all_lines len={}", all_lines.len());

        let norm = sel.normalized();
        let mut result = String::new();

        for (line_idx, line) in all_lines.iter().enumerate() {
            if line_idx < norm.start_line || line_idx > norm.end_line {
                continue;
            }

            let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

            if line_idx == norm.start_line && line_idx == norm.end_line {
                let char_count = line_text.chars().count();
                let start = norm.start_col.min(char_count);
                let end = norm.end_col.min(char_count);
                result.push_str(&substring_by_chars(&line_text, start, end));
            } else if line_idx == norm.start_line {
                let char_count = line_text.chars().count();
                let start = norm.start_col.min(char_count);
                result.push_str(&substring_by_chars(&line_text, start, char_count));
                result.push('\n');
            } else if line_idx == norm.end_line {
                let char_count = line_text.chars().count();
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

    /// Get all lines for selection extraction (without modifying state).
    fn all_lines_for_selection(&self) -> Vec<Arc<Line<'static>>> {
        let mut result = self.banner_cache.clone();
        result.extend(self.msg_lines.iter().cloned());

        // Add streaming content if any
        let has_streaming = self.is_streaming
            || !self.streaming_content.is_empty()
            || !self.streaming_thinking.is_empty();
        if !self.messages.is_empty() && has_streaming {
            result.push(Arc::new(Line::from("")));
        }
        if has_streaming {
            result.extend(self.render_streaming_static());
        }

        result
    }

    /// Static version of `render_streaming` for selection (doesn't modify self).
    fn render_streaming_static(&self) -> Vec<Arc<Line<'static>>> {
        let mut lines = Vec::new();

        // Render thinking if present
        if !self.streaming_thinking.is_empty() {
            lines.push(Arc::new(Line::from(vec![Span::styled(
                format!(
                    "Thinking ({} tokens)",
                    tokens::estimate_tokens(&self.streaming_thinking)
                ),
                Style::default()
                    .fg(colors::text_secondary())
                    .add_modifier(Modifier::ITALIC),
            )])));
        }

        // Add separator
        if !self.streaming_thinking.is_empty() && !self.streaming_content.is_empty() {
            lines.push(Arc::new(Line::from("")));
        }

        // Render content
        if !self.streaming_content.is_empty() {
            let mut md_renderer = StreamingMarkdownRenderer::new();
            md_renderer.set_content(self.streaming_content.clone());
            for line in md_renderer.lines() {
                lines.push(Arc::new(line.clone()));
            }
        }

        lines
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
        let all_lines = self.all_lines_for_selection();
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

        // Convert character position to char indices for iteration
        let char_indices: Vec<(usize, char)> = line_text.char_indices().collect();

        // Map character column to byte position
        let byte_pos = char_idx_to_byte_idx(&line_text, col);

        // Find start of word (in characters)
        let mut start_char_idx = col;
        for (idx, (byte_idx, c)) in char_indices.iter().enumerate() {
            if *byte_idx > byte_pos {
                break;
            }
            if !c.is_alphanumeric() && *c != '_' {
                start_char_idx = idx + 1;
            }
        }

        // Find end of word (in characters)
        let mut end_char_idx = col;
        for (idx, (byte_idx, c)) in char_indices.iter().enumerate() {
            if *byte_idx < byte_pos {
                continue;
            }
            if !c.is_alphanumeric() && *c != '_' {
                end_char_idx = idx;
                break;
            }
            end_char_idx = idx + 1;
        }

        self.selection = Some(Selection {
            start_line: line,
            start_col: start_char_idx,
            end_line: line,
            end_col: end_char_idx,
        });
        // Mark as selecting so copy will trigger on mouse up
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
        use tuirealm::event::MouseEventKind;

        // Check if scroll button was clicked (on Down event)
        if matches!(kind, MouseEventKind::Down(_)) && self.is_click_on_scroll_button(x, y) {
            self.scroll_to_bottom();
            return MouseAction::ScrollToBottom;
        }

        // Get width from current area for coordinate conversion
        let width = self.current_area.map_or(80, |a| a.width as usize);

        match kind {
            MouseEventKind::Down(_) => {
                if let Some((line, col)) = self.screen_to_position(x, y, width) {
                    if self.is_double_click(line, col) {
                        // Rebuild caches before selecting word
                        self.rebuild_msg_cache();
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
                    if let Some((line, col)) = self.screen_to_position(x, y, width) {
                        self.update_selection(line, col);
                    }
                }
                MouseAction::None
            }
            MouseEventKind::Up(_) => {
                if self.is_selecting {
                    self.end_selection();
                    // Rebuild caches before copying to ensure we have all lines
                    self.rebuild_msg_cache();
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
    /// Get selection as optional tuple for `WrapParagraph` rendering.
    fn get_selection_for_render(&self) -> Option<((usize, usize), (usize, usize))> {
        self.selection.map(|s| {
            let norm = s.normalized();
            (
                (norm.start_line, norm.start_col),
                (norm.end_line, norm.end_col),
            )
        })
    }
}

impl Component for ChatView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let visible_height = area.height as usize;
        let width = area.width as usize;
        self.last_visible_height = visible_height;
        self.current_area = Some(area);
        // Reset scroll button area at start of each frame
        self.scroll_button_area = None;

        // Remember previous total lines to detect content growth
        let prev_total_lines = self.total_visual_lines;

        // Build text content
        let all_lines = self.all_lines();
        let text = Text::from(all_lines.iter().map(|l| (**l).clone()).collect::<Vec<_>>());

        // Calculate and cache total visual lines
        self.total_visual_lines = WrapParagraph::new(text.clone()).wrapped_line_count(width);

        // If user has scrolled up, adjust scroll_offset to keep view stable
        // Content grew: increase offset to stay in place
        // Content shrank (streaming ended): decrease offset to stay in place
        if self.is_scrolled_up() && prev_total_lines > 0 {
            let lines_delta = self.total_visual_lines as i64 - prev_total_lines as i64;
            self.scroll_offset = (self.scroll_offset as i64 + lines_delta).max(0) as usize;
        }

        // Clamp scroll_offset to valid range
        // scroll_offset is visual lines from bottom, max is total - visible
        let max_scroll = self.total_visual_lines.saturating_sub(visible_height);
        self.scroll_offset = self.scroll_offset.min(max_scroll);

        // scroll_offset is now visual lines from bottom, convert to scroll from top
        let visual_scroll = self
            .total_visual_lines
            .saturating_sub(visible_height)
            .saturating_sub(self.scroll_offset);

        // Update viewport_lines and last_viewport for mouse coordinate conversion
        // Calculate which lines are currently visible based on visual_scroll
        let mut lines_seen = 0;
        let mut viewport_start = 0;
        for line in &all_lines {
            let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let wrapped_height = Self::calculate_wrap_boundaries(&line_text, width).len();

            if lines_seen + wrapped_height > visual_scroll {
                break;
            }
            lines_seen += wrapped_height;
            viewport_start += 1;
        }

        // Calculate how many wrap rows into the first visible line we need to skip
        let first_line_rows_before_scroll = lines_seen;
        self.viewport_first_row_offset =
            visual_scroll.saturating_sub(first_line_rows_before_scroll);

        // Collect visible lines (need enough lines to fill the screen)
        self.viewport_lines.clear();
        let mut visible_rows_needed = visible_height;
        for (i, line) in all_lines.iter().enumerate().skip(viewport_start) {
            if visible_rows_needed == 0 {
                break;
            }
            let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let wrapped_height = Self::calculate_wrap_boundaries(&line_text, width).len();

            self.viewport_lines.push((**line).clone());

            // First line contributes fewer visible rows due to scroll offset
            let rows_in_this_line = if i == viewport_start {
                wrapped_height.saturating_sub(self.viewport_first_row_offset)
            } else {
                wrapped_height
            };
            visible_rows_needed = visible_rows_needed.saturating_sub(rows_in_this_line);
        }
        self.last_viewport = (viewport_start, viewport_start + self.viewport_lines.len());

        // Render with WrapParagraph (handles wrap internally)
        let selection = self.get_selection_for_render();
        let highlight_style = Style::default()
            .fg(colors::text_primary())
            .bg(colors::selected_bg());

        let paragraph = WrapParagraph::new(text)
            .scroll((visual_scroll as u16, 0))
            .selection(selection)
            .highlight_style(highlight_style);

        frame.render_widget(paragraph, area);

        // Draw scroll-to-bottom button if user has scrolled up
        if self.is_scrolled_up() {
            self.draw_scroll_button(frame, area);
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
            attr::ADD_ERROR_MESSAGE => {
                if let AttrValue::String(error) = value {
                    self.add_error_message(error);
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
            attr::SET_BANNER => {
                if let AttrValue::String(working_dir) = value {
                    self.set_banner(crate::components::BannerData::new(working_dir));
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
                    self.complete_tool(tool_id, output, elapsed_ms, content_blocks);
                }
            }
            attr::FAIL_TOOL => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let error = parts.get(1).map_or(String::new(), |s| (*s).to_string());
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    self.fail_tool(tool_id, error, elapsed_ms);
                }
            }
            attr::UPDATE_TOOL_PROGRESS => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let message = parts.get(1).map_or(String::new(), |s| (*s).to_string());
                    let tokens = parts.get(2).and_then(|s| s.parse().ok());
                    self.update_tool_progress(&tool_id, &message, tokens);
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
                self.banner = None;
                self.queued_message = None;
            }
            attr::SET_QUEUED_MESSAGE => {
                if let AttrValue::String(blocks_json) = value {
                    if let Ok(content_blocks) =
                        serde_json::from_str::<Vec<ContentBlock>>(&blocks_json)
                    {
                        self.set_queued_message(content_blocks);
                    }
                }
            }
            attr::CLEAR_QUEUED_MESSAGE => {
                self.clear_queued_message();
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        // Return banner working_dir
        self.banner.as_ref().map_or_else(
            || State::None,
            |banner| State::Single(StateValue::String(banner.working_dir.clone())),
        )
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

    /// Initialize history from kernel messages (for session resume)
    pub fn init_history(&mut self, messages: &[kernel::types::Message]) {
        for msg in messages {
            match msg.role {
                kernel::types::Role::User => {
                    if !msg.content.is_empty() {
                        self.component.add_user_message(msg.content.clone());
                    }
                }
                kernel::types::Role::Assistant => {
                    let content = msg.text_content();
                    let thinking = msg.thinking_content();
                    self.component
                        .add_assistant_message(content, thinking, None);

                    // Handle tool calls
                    if let Some(ref tool_calls) = msg.tool_calls {
                        for call in tool_calls {
                            let args = serde_json::to_string(&call.arguments).ok();
                            self.component
                                .start_tool(call.id.clone(), call.name.clone(), args);
                        }
                    }
                }
                kernel::types::Role::Tool => {
                    if let Some(ref tool_call_id) = msg.tool_call_id {
                        let output = msg.text_content();
                        // For tool messages, we need to find the corresponding tool in history
                        // and mark it as completed. Since we don't have elapsed_ms, use 0.
                        // Content blocks are not available during history init, pass empty vec.
                        self.component
                            .complete_tool(tool_call_id.clone(), output, 0, Vec::new());
                    }
                }
                kernel::types::Role::System => {}
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
                    if let Some(messages) = any.downcast_ref::<Vec<kernel::types::Message>>() {
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
                self.component.tick();
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
                        Some(Msg::Notification(Notification::info(msg, 2000)))
                    }
                    MouseAction::None => {
                        if matches!(kind, MouseEventKind::Down(_) | MouseEventKind::Drag(_)) {
                            // Selection in progress, just redraw
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

/// Convert tool name to CamelCase for display
/// e.g., "subagent" -> "Subagent", "read" -> "Read", "`TaskCreate`" -> "`TaskCreate`"
fn to_camel_case(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // If already starts with uppercase, assume it's already CamelCase
    if s.chars().next().unwrap().is_uppercase() {
        return s.to_string();
    }

    // Convert first char to uppercase, keep rest as-is
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

fn tool_icon(tool_name: &str) -> &'static str {
    match tool_name {
        SUBAGENT_TOOL_NAME => "󰚩 ",
        READ_TOOL_NAME => " ",
        WRITE_TOOL_NAME | EDIT_TOOL_NAME => " ",
        SHELL_TOOL_NAME => " ",
        GLOB_TOOL_NAME => "󰱼 ",
        GREP_TOOL_NAME => "󰑑 ",
        SKILL_TOOL_NAME => "⚡",
        WEBFETCH_TOOL_NAME => "󰖟 ",
        WEBSEARCH_TOOL_NAME => " ",
        REMINDER_TOOL_NAME => "󰀠 ",
        // Task tools
        TASK_CREATE_TOOL_NAME
        | TASK_GET_TOOL_NAME
        | TASK_LIST_TOOL_NAME
        | TASK_UPDATE_TOOL_NAME
        | TODO_WRITE_TOOL_NAME
        | TODO_READ_TOOL_NAME => " ",
        _ => " ",
    }
}

/// Sanitize text for single-line display by replacing newlines/tabs with spaces.
fn sanitize_single_line(s: &str) -> String {
    s.replace(['\n', '\r', '\t'], " ")
}



/// Extract a concise description from tool arguments for the title
/// e.g., Read "src/main.rs", Edit "crates/tui/src/lib.rs"
/// Results are truncated to 100 characters (Unicode-safe).
fn extract_tool_target(tool_name: &str, args: Option<&str>) -> Option<String> {
    const MAX_LEN: usize = 100;
    let args = args?;
    let value = serde_json::from_str::<serde_json::Value>(args).ok()?;

    let f = |s: &str| truncate_by_chars(&sanitize_single_line(s), MAX_LEN);

    let target = match tool_name {
        READ_TOOL_NAME | EDIT_TOOL_NAME => {
            value["path"].as_str().map(|path| {
                // For skill files, show the parent directory name
                if path.ends_with(SKILL_FILENAME) {
                    std::path::Path::new(path)
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .map_or_else(|| f(path), |s| format!("{s}/{SKILL_FILENAME}"))
                } else {
                    f(path)
                }
            })
        }
        WRITE_TOOL_NAME => value["file_path"].as_str().map(f),
        SHELL_TOOL_NAME => value["command"].as_str().map(f),
        GLOB_TOOL_NAME | GREP_TOOL_NAME => value["pattern"].as_str().map(f),
        WEBFETCH_TOOL_NAME => value["url"].as_str().map(f),
        SKILL_TOOL_NAME => value["name"]
            .as_str()
            .map(f)
            .or_else(|| value["path"].as_str().map(f)),
        SUBAGENT_TOOL_NAME => value["prompt"].as_str().map(f),
        TODO_READ_TOOL_NAME => Some(String::new()), // No specific target for reading todos
        _ => None,
    };

    target.map(|t| truncate_by_chars(&sanitize_single_line(&t), MAX_LEN))
}
