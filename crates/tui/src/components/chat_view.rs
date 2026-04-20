//! Unified chat view component
//!
//! Displays chat history + streaming message in a single scrollable view.

use std::sync::Arc;

use tuirealm::{
    command::{Cmd, CmdResult},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::Rect,
        style::{Modifier, Style},
        text::{Line, Span, Text},
        widgets::Paragraph,
    },
    Component, Frame, MockComponent, State,
};

use crate::{
    markdown_stream::StreamingMarkdownRenderer,
    msg::Msg,
    theme::colors,
    utils::text::truncate_unicode,
    utils::{strs, text::preprocess},
};
use kernel::utils::tokens;
use kernel::tools::{
    BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, READ_TOOL_NAME,
    SKILL_TOOL_NAME, SUBAGENT_TOOL_NAME, WRITE_TOOL_NAME,
};
use kernel::task::{
    TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME, TASK_UPDATE_TOOL_NAME,
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

/// A chat message in history
#[derive(Debug, Clone)]
pub enum HistoryMessage {
    User(String),
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
    },
    Error(String),
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
    // Track if user manually scrolled up (to pause auto-scroll)
    user_scrolled: bool,
    // Track active tool executions
    active_tools: std::collections::HashMap<String, (String, ToolStatus)>, // tool_id -> (tool_name, status)
    // Expand all mode (ctrl-o): show all thinking and tool details
    expand_all: bool,
    // Banner data (rendered as first content, scrolls with messages)
    banner: Option<crate::components::BannerData>,
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
            user_scrolled: false,
            active_tools: std::collections::HashMap::new(),
            expand_all: false,
            banner: Some(crate::components::BannerData::default()),
            mascot_animator: MascotAnimator::default(),
            last_visible_height: 0,
            msg_cache: Vec::new(),
            banner_cache: Vec::new(),
            banner_dirty: true,
            msg_lines: Vec::new(),
            msg_cache_dirty: true,
            viewport_lines: Vec::new(),
            last_viewport: (0, 0),
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
        self.msg_cache_dirty = true;
        self.banner_dirty = true;
    }

    /// Set banner data to display at the top
    pub fn set_banner(&mut self, banner: crate::components::BannerData) {
        self.banner = Some(banner);
        self.banner_dirty = true;
    }

    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(HistoryMessage::User(content));
        self.push_new_msg_cache();
        // Auto scroll to bottom on new message
        self.scroll_to_bottom();
    }

    pub fn add_error_message(&mut self, error: String) {
        self.messages.push(HistoryMessage::Error(error));
        self.push_new_msg_cache();
        // Auto scroll to bottom on new message
        self.scroll_to_bottom();
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
        // Auto scroll to bottom on new message
        self.scroll_to_bottom();
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
        });
        self.push_new_msg_cache();
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    pub fn complete_tool(&mut self, tool_id: String, output: String, elapsed_ms: u64) {
        // Update the tool message in history and invalidate cache
        for (i, msg) in self.messages.iter_mut().enumerate().rev() {
            if let HistoryMessage::Tool {
                tool_id: id,
                status,
                output: out,
                elapsed_ms: elapsed,
                ..
            } = msg
            {
                if id == &tool_id {
                    *status = ToolStatus::Completed;
                    *out = Some(output);
                    *elapsed = Some(elapsed_ms);
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
        if !self.user_scrolled {
            self.scroll_offset = 0;
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
        if !self.user_scrolled {
            self.scroll_offset = 0;
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
        self.scroll_offset = 0;
        // Reset user scrolled state for new streaming session
        self.user_scrolled = false;
    }

    pub fn stop_streaming(&mut self) {
        self.is_streaming = false;
    }

    pub fn clear_streaming(&mut self) {
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
        // Auto scroll to bottom only if user hasn't manually scrolled up
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
        // Streaming content affects rendered output
        self.msg_cache_dirty = true;
    }

    pub fn append_streaming_thinking(&mut self, text: &str) {
        self.streaming_thinking.push_str(text);
        // Auto scroll to bottom only if user hasn't manually scrolled up
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
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
        let total_lines = self.all_lines_len();
        // Use last_visible_height to calculate reasonable max_scroll
        // Ensure we can always see at least 1 line when at top
        let visible = self.last_visible_height.saturating_sub(1).max(1);
        let max_scroll = total_lines.saturating_sub(visible);

        // If already at or near top, don't increase offset further
        if self.scroll_offset >= max_scroll {
            self.scroll_offset = max_scroll;
            self.user_scrolled = true;
            return;
        }

        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
        // User manually scrolled up, pause auto-scroll
        self.user_scrolled = true;
    }

    pub fn scroll_down(&mut self, amount: usize) {
        // Accelerate scrolling when offset is large to quickly return to bottom
        let accelerated = if self.scroll_offset > 100 {
            amount.saturating_mul(5) // 5x speed when far from bottom
        } else if self.scroll_offset > 50 {
            amount.saturating_mul(3) // 3x speed when moderately far
        } else {
            amount
        };

        self.scroll_offset = self.scroll_offset.saturating_sub(accelerated);
        // If scrolled to bottom, resume auto-scroll
        if self.scroll_offset == 0 {
            self.user_scrolled = false;
        }
    }

    pub const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        // User scrolled to bottom, resume auto-scroll
        self.user_scrolled = false;
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = self.accurate_lines_len();
        // User manually scrolled, pause auto-scroll
        self.user_scrolled = true;
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

    /// Get scroll progress for browse mode (`current_line`, `total_lines`)
    /// Returns the 1-based current visible start position and total lines
    pub fn get_scroll_progress(&self) -> (usize, usize) {
        let total_lines = self.all_lines_len();
        if total_lines == 0 {
            return (0, 0);
        }

        // Calculate current visible start line (1-based) based on scroll_offset
        // scroll_offset = 0 means at bottom showing latest content
        // scroll_offset > 0 means scrolled up by that many lines from bottom
        let start_line = if self.scroll_offset == 0 {
            // At bottom: show the last visible_height lines
            total_lines
                .saturating_sub(self.last_visible_height.saturating_sub(1))
                .max(1)
        } else {
            // Scrolled up: start_line is scroll_offset lines from bottom
            total_lines.saturating_sub(self.scroll_offset).max(1)
        };

        (start_line.min(total_lines), total_lines)
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

    pub fn page_up(&mut self, page_height: usize) {
        let amount = page_height.saturating_sub(2); // Leave some context
        self.scroll_up(amount);
    }

    pub fn page_down(&mut self, page_height: usize) {
        let amount = page_height.saturating_sub(2); // Leave some context
        self.scroll_down(amount);
    }

    #[allow(clippy::cast_precision_loss)]
    fn render_message(msg: &HistoryMessage) -> Vec<Arc<Line<'static>>> {
        let mut lines = Vec::new();

        match msg {
            HistoryMessage::User(content) => {
                let user_bg = colors::user_msg_bg();
                for (i, line) in content.lines().enumerate() {
                    let prefix = if i == 0 { "❯ " } else { "│ " };
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
                status,
                output,
                error,
                folded,
                arguments,
                elapsed_ms,
                ref tokens,
                ref progress,
                ..
            } => {
                let color = match status {
                    ToolStatus::Running => colors::accent_warning(),
                    ToolStatus::Completed => colors::accent_success(),
                    ToolStatus::Failed => colors::accent_error(),
                    ToolStatus::Cancelled => colors::text_secondary(),
                };
                let icon = toolname_to_icon(tool_name);

                // Build header with execution time (only show if >= 1s)
                let time_str = elapsed_ms
                    .filter(|ms| *ms >= 1000)
                    .map(|ms| format!(" {:.1}s", ms as f64 / 1000.0))
                    .unwrap_or_default();

                // Peek args in folded mode (max 30 chars, compact whitespace)
                let peek_args = if *folded {
                    arguments.as_ref().and_then(|args| {
                        // Compact whitespace: replace newlines/tabs with single space
                        let compact = args;
                        if compact.is_empty() {
                            None
                        } else {
                            let peek = strs::truncate_with_suffix(compact, 80, "...");
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
                lines.push(Arc::new(Line::from(header_spans)));

                // Output peek in folded mode (max 50 chars, indented)
                if *folded {
                    // Show progress for running tools
                    if *status == ToolStatus::Running {
                        if let Some(ref prog) = progress {
                            let prog_text = prog.clone();
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

                    let peek_output = error.as_ref().or(output.as_ref()).and_then(|out| {
                        let trimmed = out.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            let peek = strs::truncate_with_suffix(trimmed, 200, "...");
                            Some(peek.split_whitespace().collect::<Vec<_>>().join(" "))
                        }
                    });
                    if let Some(peek) = peek_output {
                        lines.push(Arc::new(Line::from(vec![
                            Span::styled(" ⎿ ", Style::default().fg(colors::text_secondary())),
                            Span::styled(peek, Style::default().fg(colors::text_secondary())),
                        ])));
                    }
                }

                if !*folded {
                    // Show tool arguments if available
                    if let Some(args) = arguments {
                        if !args.is_empty() {
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled("│ ", Style::default().fg(colors::text_secondary())),
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
                                        "│   ",
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
                                Span::styled("│ ", Style::default().fg(colors::accent_error())),
                                Span::styled(
                                    preprocess(line),
                                    Style::default().fg(colors::accent_error()),
                                ),
                            ])));
                        }
                    } else if let Some(out) = output {
                        lines.push(Arc::new(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(colors::text_secondary())),
                            Span::styled(
                                "Output:",
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])));
                        for line in out.lines() {
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled("│ ", Style::default().fg(colors::accent_system())),
                                Span::styled(
                                    preprocess(line),
                                    Style::default().fg(colors::text_primary()),
                                ),
                            ])));
                        }
                    } else if *status == ToolStatus::Running {
                        let running_text = progress
                            .as_ref()
                            .map_or_else(|| "Running...".to_string(), |p| format!("Running: {p}"));
                        lines.push(Arc::new(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(colors::text_secondary())),
                            Span::styled(
                                running_text,
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ])));
                    } else if *status == ToolStatus::Cancelled {
                        lines.push(Arc::new(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(colors::text_secondary())),
                            Span::styled(
                                "Cancelled",
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ])));
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
                    Span::styled("│ ", Style::default().fg(colors::text_secondary())),
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

    /// Sync `msg_cache` length with messages length.
    fn sync_msg_cache(&mut self) {
        while self.msg_cache.len() < self.messages.len() {
            self.msg_cache.push(None);
        }
        if self.msg_cache.len() > self.messages.len() {
            self.msg_cache.truncate(self.messages.len());
        }
    }

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
                let info_part = info_lines.get(i).map_or("", |s| s.as_str());
                let mascot_padded = format!("{mascot_part:width$}", width = Self::MASCOT_COL_WIDTH);

                self.banner_cache.push(Arc::new(Line::from(vec![
                    Span::styled(mascot_padded, colors::accent_system()),
                    Span::styled(info_part.to_string(), colors::text_secondary()),
                ])));
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

        for (i, msg) in self.messages.iter().enumerate() {
            let msg_lines = match &self.msg_cache[i] {
                Some(lines) => lines,
                None => {
                    let rendered = Self::render_message(msg);
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

    /// Get total line count (uses cached values, may be stale if dirty).
    /// Note: This is called from scroll functions. For accurate count, ensure caches are rebuilt first.
    fn all_lines_len(&self) -> usize {
        let banner_len = self.banner_cache.len();
        let msg_len = self.msg_lines.len();
        // Note: streaming is not included in cached count since it changes frequently
        banner_len + msg_len
    }

    /// Get accurate total line count (includes streaming, rebuilds caches if needed).
    fn accurate_lines_len(&mut self) -> usize {
        self.all_lines().len()
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

        result
    }

    /// Get visible lines for the current viewport, rebuilding cache if needed.
    fn get_lines(&mut self, visible_height: usize, width: usize) -> Vec<Line<'static>> {
        self.sync_msg_cache();
        let all_lines = self.all_lines();

        let (start_line, end_line) = self.calculate_viewport(&all_lines, visible_height, width);
        self.update_viewport_cache(&all_lines, start_line, end_line);

        // Pad with empty lines to fill the entire area
        let mut result = self.viewport_lines.clone();
        while result.len() < visible_height {
            result.push(Line::from(""));
        }
        result
    }

    /// Calculate viewport range (`start_line`, `end_line`) based on scroll state.
    fn calculate_viewport(
        &self,
        all_lines: &[Arc<Line<'static>>],
        visible_height: usize,
        width: usize,
    ) -> (usize, usize) {
        let total_lines = all_lines.len();
        if total_lines == 0 {
            return (0, 0);
        }

        let start_line = if self.scroll_offset == 0 {
            // At bottom: work backwards to find which lines fit
            let mut wrapped_lines = 0;
            let mut start = 0;
            for (i, line) in all_lines.iter().enumerate().rev() {
                let line_width: usize = line
                    .spans
                    .iter()
                    .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                let wrapped_height = (line_width + width.saturating_sub(1)) / width.max(1);
                let wrapped_height = wrapped_height.max(1);

                if wrapped_lines + wrapped_height > visible_height {
                    start = i + 1;
                    break;
                }
                wrapped_lines += wrapped_height;
                if i == 0 {
                    start = 0;
                    break;
                }
            }
            start
        } else {
            // Manual scroll: use simple line-based calculation
            if total_lines > visible_height + self.scroll_offset {
                total_lines - visible_height - self.scroll_offset
            } else {
                0
            }
        };

        // When at bottom, show all lines from start to end
        // When scrolling, limit to visible_height
        let end_line = if self.scroll_offset == 0 {
            total_lines
        } else {
            (start_line + visible_height).min(total_lines)
        };
        (start_line, end_line)
    }

    /// Update viewport cache when viewport range or content changes.
    ///
    /// NOTE: This always clones the visible lines rather than using a dirty check,
    /// because content can change without the viewport range changing (e.g., streaming
    /// text updates). The performance cost is negligible for typical terminal sizes
    /// (~30-50 lines). If profiling shows this is a bottleneck, consider adding a
    /// `viewport_dirty` flag to track content changes and skip unnecessary clones.
    fn update_viewport_cache(
        &mut self,
        all_lines: &[Arc<Line<'static>>],
        start_line: usize,
        end_line: usize,
    ) {
        // Always update because content may have changed even if range is same
        self.viewport_lines.clear();
        for arc_line in &all_lines[start_line..end_line] {
            self.viewport_lines.push((**arc_line).clone());
        }
        self.last_viewport = (start_line, end_line);
    }
}

impl MockComponent for ChatView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let visible_height = area.height as usize;
        self.last_visible_height = visible_height;

        let lines = self.get_lines(visible_height, area.width as usize);
        let paragraph = Paragraph::new(Text::from(lines))
            .wrap(tuirealm::ratatui::widgets::Wrap { trim: false });

        frame.render_widget(paragraph, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        match attr {
            Attribute::Custom("scroll_progress") => {
                let (current, total) = self.get_scroll_progress();
                Some(AttrValue::String(format!("{current}\x00{total}")))
            }
            _ => self.props.get(attr),
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        // Extract the custom string first, then match on it
        let Attribute::Custom(cmd) = attr else {
            self.props.set(attr, value);
            return;
        };

        match cmd {
            "add_user_message" => {
                if let AttrValue::String(content) = value {
                    self.add_user_message(content);
                }
            }
            "add_error_message" => {
                if let AttrValue::String(error) = value {
                    self.add_error_message(error);
                }
            }
            "add_assistant_with_thinking" => {
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
            "set_banner" => {
                if let AttrValue::String(banner_str) = value {
                    let parts: Vec<&str> = banner_str.split('\x00').collect();
                    let working_dir = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let skills = parts.get(1).map_or(Vec::new(), |s| {
                        s.split(',').map(|skill| skill.trim().to_string()).collect()
                    });
                    self.set_banner(crate::components::BannerData::new(working_dir, skills));
                }
            }
            "start_streaming" => self.start_streaming(),
            "stop_streaming" => self.stop_streaming(),
            "clear_streaming" => self.clear_streaming(),
            "cancel_streaming" => self.cancel_streaming(),
            "cancel_streaming_with_content" => {
                if let AttrValue::String(combined) = value {
                    let parts: Vec<&str> = combined.split('\x00').collect();
                    let content = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let thinking = parts
                        .get(1)
                        .filter(|s| !s.is_empty())
                        .map(|s| (*s).to_string());

                    // Add to history with cancelled flag
                    self.messages.push(HistoryMessage::Assistant {
                        content,
                        thinking,
                        thinking_folded: !self.expand_all,
                        thinking_elapsed_ms: parts.get(2).and_then(|s| s.parse().ok()),
                    });
                    self.push_new_msg_cache();

                    // Mark running tools as cancelled and invalidate their caches
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
                        if let Some(HistoryMessage::Tool { status, .. }) =
                            self.messages.get_mut(idx)
                        {
                            *status = ToolStatus::Cancelled;
                        }
                        self.invalidate_msg_cache(idx);
                    }

                    // Clear streaming state
                    self.streaming_content.clear();
                    self.streaming_thinking.clear();
                    self.md_renderer = StreamingMarkdownRenderer::new();
                    self.is_streaming = false;
                }
            }
            "append_content" => {
                if let AttrValue::String(text) = value {
                    self.append_streaming_content(&text);
                }
            }
            "append_thinking" => {
                if let AttrValue::String(text) = value {
                    self.append_streaming_thinking(&text);
                }
            }
            "scroll_up" => self.scroll_up(3),
            "scroll_down" => self.scroll_down(3),
            "scroll_to_bottom" => self.scroll_to_bottom(),
            "scroll_to_top" => self.scroll_to_top(),
            "toggle_thinking" => self.toggle_last_thinking(),
            "toggle_expand_all" => self.toggle_expand_all(),
            "expand_all" => self.expand_all(),
            "collapse_all" => self.collapse_all(),
            "start_tool" => {
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
            "complete_tool" | "fail_tool" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let second = parts.get(1).map_or(String::new(), |s| (*s).to_string());
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    match cmd {
                        "complete_tool" => self.complete_tool(tool_id, second, elapsed_ms),
                        "fail_tool" => self.fail_tool(tool_id, second, elapsed_ms),
                        _ => {}
                    }
                }
            }
            "update_tool_progress" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.first().map_or(String::new(), |s| (*s).to_string());
                    let message = parts.get(1).map_or(String::new(), |s| (*s).to_string());
                    let tokens = parts.get(2).and_then(|s| s.parse().ok());
                    self.update_tool_progress(&tool_id, &message, tokens);
                }
            }
            "page_up" | "page_down" => {
                if let AttrValue::Number(height) = value {
                    match cmd {
                        "page_up" => self.page_up(height as usize),
                        "page_down" => self.page_down(height as usize),
                        _ => {}
                    }
                }
            }
            "clear_history" => {
                self.clear_all_caches();
                self.messages.clear();
                self.scroll_offset = 0;
                self.banner = None;
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        // Return banner data if present: "working_dir|skill1,skill2,..."
        self.banner.as_ref().map_or_else(
            || State::None,
            |banner| {
                let banner_str = format!("{}\x00{}", banner.working_dir, banner.skills.join(","));
                State::One(tuirealm::StateValue::String(banner_str))
            },
        )
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(tuirealm::command::Direction::Up) => {
                self.scroll_up(1);
                CmdResult::None
            }
            Cmd::Move(tuirealm::command::Direction::Down) => {
                self.scroll_down(1);
                CmdResult::None
            }
            _ => CmdResult::None,
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
                    let text = msg.text_content();
                    if !text.is_empty() {
                        self.component.add_user_message(text);
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
                        self.component
                            .complete_tool(tool_call_id.clone(), output, 0);
                    }
                }
                kernel::types::Role::System => {}
            }
        }
    }
}

impl MockComponent for ChatViewComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.component.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.component.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        use tuirealm::props::PropPayload;
        match attr {
            Attribute::Custom("init_history") => {
                if let AttrValue::Payload(PropPayload::Any(payload)) = value {
                    use tuirealm::props::PropBoundExt;
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

impl Component<Msg, crate::msg::UserEvent> for ChatViewComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        // Keyboard events are handled at app level via InputComponent
        // Only handle Tick here for the blinking indicator
        if ev == tuirealm::Event::Tick {
            self.component.tick();
            Some(Msg::Redraw)
        } else {
            None
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

fn toolname_to_icon(tool_name: &str) -> &'static str {
    match tool_name.to_lowercase().as_str() {
        n if n == SUBAGENT_TOOL_NAME => "󰚩 ",
        n if n == READ_TOOL_NAME => " ",
        n if n == WRITE_TOOL_NAME || n == EDIT_TOOL_NAME => " ",
        n if n == BASH_TOOL_NAME => " ",
        n if n == GLOB_TOOL_NAME => "󰱼 ",
        n if n == GREP_TOOL_NAME => " ",
        n if n == SKILL_TOOL_NAME => "⚡",
        // Task tools
        n if n == TASK_CREATE_TOOL_NAME
            || n == TASK_GET_TOOL_NAME
            || n == TASK_LIST_TOOL_NAME
            || n == TASK_UPDATE_TOOL_NAME => " ",
        _ => " ",
    }
}

/// Extract a concise description from tool arguments for the title
/// e.g., Read "src/main.rs", Edit "crates/tui/src/lib.rs"
/// Results are truncated to 100 characters (Unicode-safe).
fn extract_tool_target(tool_name: &str, args: Option<&str>) -> Option<String> {
    const MAX_LEN: usize = 100;
    let args = args?;
    let value = serde_json::from_str::<serde_json::Value>(args).ok()?;

    let target = match tool_name.to_lowercase().as_str() {
        "read" | "edit" => value["path"].as_str().map(String::from),
        "write" => value["file_path"].as_str().map(String::from),
        "bash" => {
            let cmd = value["command"].as_str()?;
            let cmd_display = truncate_unicode(cmd, 50); // Reserve space for suffix

            let timeout_secs = value["timeout"].as_u64();
            let background = value["background"].as_bool().unwrap_or(false);

            // Build suffix like [async, 120s] or [60s]
            let mut parts = Vec::new();
            if background {
                parts.push("async".to_string());
            }
            if let Some(t) = timeout_secs {
                // Only show timeout if explicitly set or background mode
                if background || t != 60 {
                    parts.push(format!("{t}s"));
                }
            }

            if parts.is_empty() {
                Some(cmd_display)
            } else {
                Some(format!("{} [{}]", cmd_display, parts.join(", ")))
            }
        }
        "glob" | "grep" => value["pattern"].as_str().map(String::from),
        n if n == SUBAGENT_TOOL_NAME => value["prompt"]
            .as_str()
            .map(|p| truncate_unicode(p, MAX_LEN)),
        _ => None,
    };

    // Apply unicode-safe truncation to all results
    target.map(|t| truncate_unicode(&t, MAX_LEN))
}
