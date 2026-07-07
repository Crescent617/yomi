//! Message rendering: pure functions that convert `HistoryMessage` / streaming content
//! into ratatui Lines.
//!
//! This module is intentionally stateless. All rendering decisions are passed as parameters.

use std::sync::Arc;

use tuirealm::ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::components::chat_view::{HistoryMessage, SubagentState, ToolStatus};
use crate::markdown_stream::StreamingMarkdownRenderer;
use crate::theme::{chars, colors};
use crate::utils::text::{preprocess, truncate_by_chars, truncate_by_width};

use kernel::tools::{
    task::{TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME, TASK_UPDATE_TOOL_NAME},
    ASK_USER_TOOL_NAME,
};
use kernel::tools::{
    EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, READ_TOOL_NAME, REMINDER_TOOL_NAME,
    SHELL_TOOL_NAME, SKILL_FILENAME, SKILL_TOOL_NAME, SLEEP_TOOL_NAME, SUBAGENT_TOOL_NAME,
    TODO_TOOL_NAME, WEBFETCH_TOOL_NAME, WEBSEARCH_TOOL_NAME, WRITE_TOOL_NAME,
};
use kernel::types::{ContentBlock, ToolOutputBlock};
use kernel::utils::tokens;

/// Line-level diff: returns a list of (type, text) pairs.
fn diff_lines(old_str: &str, new_str: &str) -> Vec<(&'static str, String)> {
    let old_lines: Vec<&str> = old_str.split('\n').collect();
    let new_lines: Vec<&str> = new_str.split('\n').collect();
    let m = old_lines.len();
    let n = new_lines.len();

    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if old_lines[i - 1] == new_lines[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_lines[i - 1] == new_lines[j - 1] {
            result.push(("context", old_lines[i - 1].to_string()));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            result.push(("add", new_lines[j - 1].to_string()));
            j -= 1;
        } else {
            result.push(("del", old_lines[i - 1].to_string()));
            i -= 1;
        }
    }
    result.reverse();
    result
}

#[allow(clippy::cast_precision_loss)]
pub fn render_message(msg: &HistoryMessage, width: usize) -> Vec<Arc<Line<'static>>> {
    match msg {
        HistoryMessage::User(blocks) => render_user(blocks),
        HistoryMessage::Assistant {
            content,
            thinking,
            thinking_folded,
            thinking_elapsed_ms,
        } => render_assistant(
            content,
            thinking.as_deref(),
            *thinking_folded,
            *thinking_elapsed_ms,
        ),
        HistoryMessage::Tool {
            tool_name,
            tool_id: _,
            status,
            output,
            error,
            folded,
            arguments,
            parsed_args,
            elapsed_ms,
            content_blocks,
            subagent,
        } => render_tool(
            tool_name,
            status,
            output.as_deref(),
            error.as_deref(),
            *folded,
            arguments.as_deref(),
            parsed_args.as_ref(),
            *elapsed_ms,
            content_blocks,
            subagent.as_ref(),
            width,
        ),
        HistoryMessage::Error(error) => render_error(error),
        HistoryMessage::Notice(text) => render_notice(text),
    }
}

fn render_user(content_blocks: &[ContentBlock]) -> Vec<Arc<Line<'static>>> {
    let mut lines = Vec::new();

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
    lines
}

fn render_assistant(
    content: &str,
    thinking: Option<&str>,
    thinking_folded: bool,
    thinking_elapsed_ms: Option<u64>,
) -> Vec<Arc<Line<'static>>> {
    let mut lines = Vec::new();

    // Render thinking summary (folded) or detail (expanded)
    let thinking_lines = thinking
        .as_ref()
        .map(|t| render_thinking_lines(t, thinking_folded, thinking_elapsed_ms))
        .unwrap_or_default();
    let thinking_rendered = !thinking_lines.is_empty();
    lines.extend(thinking_lines);

    // Add separator between thinking and content if both exist
    if thinking_rendered && !content.is_empty() {
        lines.push(Arc::new(Line::from("")));
    }

    // Render content with markdown (no indicator)
    // Note: no empty line here, thinking already adds one if present
    if !content.is_empty() {
        let mut md_renderer = StreamingMarkdownRenderer::new();
        md_renderer.set_content(content.to_string());
        let md_lines = md_renderer.lines();

        for line in md_lines {
            lines.push(Arc::new(line.clone()));
        }
    }
    lines
}

#[allow(clippy::cast_precision_loss)]
#[allow(clippy::too_many_arguments)]
fn render_tool(
    tool_name: &str,
    status: &ToolStatus,
    output: Option<&str>,
    error: Option<&str>,
    folded: bool,
    arguments: Option<&str>,
    parsed_args: Option<&serde_json::Value>,
    elapsed_ms: Option<u64>,
    content_blocks: &[ToolOutputBlock],
    subagent: Option<&SubagentState>,
    width: usize,
) -> Vec<Arc<Line<'static>>> {
    let mut lines = Vec::new();

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
    let peek_args = if folded {
        arguments.and_then(|args| {
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
    let target = extract_tool_target(tool_name, arguments);

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
        if let Some(value) = parsed_args {
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

    // For grep, show output mode with text_secondary style
    if tool_name == GREP_TOOL_NAME {
        if let Some(value) = parsed_args {
            let mode = value["output_mode"].as_str().unwrap_or("filename");
            header_spans.push(Span::styled(
                format!(" {mode}"),
                Style::default().fg(colors::text_secondary()),
            ));
        }
    }

    // For subagent, show preset with text_secondary style
    if tool_name == SUBAGENT_TOOL_NAME {
        if let Some(value) = parsed_args {
            let preset = value["preset"].as_str().unwrap_or("general-purpose");
            header_spans.push(Span::styled(
                format!(" {preset}"),
                Style::default().fg(colors::text_secondary()),
            ));
        }
    }

    lines.push(Arc::new(Line::from(header_spans)));

    // Output peek in folded mode (max 50 chars, indented)
    if folded {
        // Show output peek in folded mode (max 2 lines based on width)
        if let Some(out) = error.or(output) {
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
    } else {
        // Show tool arguments if available
        if let Some(args) = arguments {
            if !args.is_empty() {
                if tool_name == EDIT_TOOL_NAME {
                    // Special diff view for edit tool
                    let mut diff_rendered = false;
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(args) {
                        if let (Some(old_str), Some(new_str)) =
                            (parsed["old_str"].as_str(), parsed["new_str"].as_str())
                        {
                            diff_rendered = true;
                            for (ty, text) in diff_lines(old_str, new_str) {
                                let (sign, fg) = match ty {
                                    "add" => ("+", colors::accent_success()),
                                    "del" => ("−", colors::accent_error()),
                                    _ => (" ", colors::text_secondary()),
                                };
                                lines.push(Arc::new(Line::from(vec![
                                    Span::styled(
                                        chars::MSG_INDENT2_GUIDE,
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                    Span::styled(format!("{sign} "), Style::default().fg(fg)),
                                    Span::styled(preprocess(text), Style::default().fg(fg)),
                                ])));
                            }
                        }
                    }
                    if !diff_rendered {
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
                } else {
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
            lines.push(Arc::new(Line::from(vec![
                Span::styled(
                    chars::MSG_INDENT_GUIDE,
                    Style::default().fg(colors::text_secondary()),
                ),
                Span::styled(
                    "Running...",
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
                    Span::styled(url_display, Style::default().fg(colors::text_primary())),
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

    // Render inline subagent progress if present
    if let Some(sa) = subagent {
        lines.extend(render_subagent_inline(sa, width));
    }

    lines
}

/// Render a subagent's real-time progress inline inside its parent tool card.
fn render_subagent_inline(sa: &SubagentState, _width: usize) -> Vec<Arc<Line<'static>>> {
    let mut lines = Vec::new();
    let guide = chars::MSG_INDENT_GUIDE;
    let guide_style = Style::default().fg(colors::text_secondary());

    // Status icon: dim everything when finished (Running stays accent).
    let (status_icon, status_color) = match sa.status {
        crate::components::chat_view::SubagentStatus::Running => ("󰔟", colors::accent_warning()),
        crate::components::chat_view::SubagentStatus::Completed => ("", colors::text_secondary()),
        crate::components::chat_view::SubagentStatus::Failed => ("", colors::text_secondary()),
        crate::components::chat_view::SubagentStatus::Cancelled => ("", colors::text_secondary()),
    };

    let total_tokens = sa.total_prompt_tokens + sa.total_completion_tokens;

    if sa.folded {
        // Folded: show a single line combining latest activity + tokens + session_id.
        let summary = sa
            .events
            .iter()
            .rev()
            .find_map(|ev| match ev {
                kernel::event::Event::Tool(kernel::event::ToolEvent::Start {
                    tool_name, ..
                }) => Some(tool_name.as_str()),
                _ => None,
            })
            .unwrap_or("Running…");
        let mut spans = vec![
            Span::styled(guide, guide_style),
            Span::styled(
                format!("{status_icon} {summary}"),
                Style::default().fg(status_color),
            ),
        ];
        if total_tokens > 0 {
            spans.push(Span::styled(
                format!(
                    " · {} tokens",
                    kernel::utils::tokens::format_actual_tokens(total_tokens)
                ),
                Style::default().fg(status_color),
            ));
        }
        spans.push(Span::styled(
            format!(" · {}", sa.session_id),
            Style::default().fg(colors::text_secondary()),
        ));
        lines.push(Arc::new(Line::from(spans)));
    } else {
        // Header line (always shown): description + accumulated tokens + session_id
        let mut header_spans = vec![
            Span::styled(guide, guide_style),
            Span::styled(
                format!("{status_icon} {}", sa.description),
                Style::default().fg(status_color),
            ),
        ];
        if total_tokens > 0 {
            header_spans.push(Span::styled(
                format!(
                    " · {} tokens",
                    kernel::utils::tokens::format_actual_tokens(total_tokens)
                ),
                Style::default().fg(status_color),
            ));
        }
        header_spans.push(Span::styled(
            format!(" · {}", sa.session_id),
            Style::default().fg(colors::text_secondary()),
        ));
        lines.push(Arc::new(Line::from(header_spans)));

        // Show last few events as a simple activity log.
        // NOTE: ModelEvent::Chunk is filtered out in event_pump.rs to avoid
        // TUI spam, so we only show structural events (tool start/end, lifecycle).
        let event_lines: Vec<String> = sa
            .events
            .iter()
            .rev()
            .take(8)
            .map(|ev| match ev {
                kernel::event::Event::Tool(kernel::event::ToolEvent::Start {
                    tool_name,
                    arguments,
                    ..
                }) => {
                    let target = extract_tool_target(tool_name, arguments.as_deref());
                    if let Some(t) = target {
                        format!("{tool_name} {t}")
                    } else {
                        tool_name.clone()
                    }
                }
                kernel::event::Event::Tool(kernel::event::ToolEvent::End {
                    tool_name,
                    is_error,
                    ..
                }) => {
                    if *is_error {
                        format!(" {tool_name}")
                    } else {
                        format!(" {tool_name}")
                    }
                }
                kernel::event::Event::Agent(kernel::event::AgentEvent::Lifecycle {
                    state: kernel::event::AgentStatus::Stopped { reason },
                    ..
                }) => match reason {
                    kernel::event::StopReason::Completed { .. } => " Agent completed".to_string(),
                    kernel::event::StopReason::Cancelled { .. } => " Cancelled".to_string(),
                    kernel::event::StopReason::Failed { error } => format!(" Failed: {error}"),
                    kernel::event::StopReason::MaxIterations { reached } => {
                        format!(" Max iterations ({reached})")
                    }
                },
                _ => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect();

        for line in event_lines.into_iter().rev() {
            lines.push(Arc::new(Line::from(vec![
                Span::styled(guide, guide_style),
                Span::styled(line, Style::default().fg(colors::text_secondary())),
            ])));
        }
    }

    lines
}

fn render_error(error: &str) -> Vec<Arc<Line<'static>>> {
    let mut lines = Vec::new();

    // Render error message with red color
    for line in error.lines() {
        lines.push(Arc::new(Line::from(vec![Span::styled(
            preprocess(line),
            Style::default().fg(colors::accent_error()),
        )])));
    }
    lines
}

fn render_notice(text: &str) -> Vec<Arc<Line<'static>>> {
    let mut lines = Vec::new();

    // Render notice with success green + italic
    for line in text.lines() {
        lines.push(Arc::new(Line::from(vec![Span::styled(
            preprocess(line),
            Style::default()
                .fg(colors::accent_success())
                .add_modifier(Modifier::ITALIC),
        )])));
    }
    lines
}

/// Extract text content from content blocks.
pub fn extract_text_from_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => {
                if text.is_empty() {
                    None
                } else {
                    Some(text.as_str())
                }
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render queued message to be displayed at the bottom during streaming
pub fn render_queued_message(blocks: &[ContentBlock]) -> Vec<Arc<Line<'static>>> {
    const MAX_LINES: usize = 3;
    let mut lines = Vec::new();

    let text_content = extract_text_from_blocks(blocks);

    // Render header with indicator
    lines.push(Arc::new(Line::from(vec![Span::styled(
        "󰔟 Queued (will send when streaming ends)",
        Style::default()
            .fg(colors::text_secondary())
            .add_modifier(Modifier::ITALIC),
    )])));

    // Render content with dimmed style (max 3 lines, show ... if more)
    for (i, line) in text_content.lines().take(MAX_LINES + 1).enumerate() {
        if i >= MAX_LINES {
            lines.push(Arc::new(Line::from(vec![
                Span::styled(
                    chars::MSG_INDENT_GUIDE,
                    Style::default().fg(colors::text_secondary()),
                ),
                Span::styled("...", Style::default().fg(colors::text_secondary())),
            ])));
            break;
        }
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
pub fn render_thinking_lines(
    thinking: &str,
    is_folded: bool,
    elapsed_ms: Option<u64>,
) -> Vec<Arc<Line<'static>>> {
    if thinking.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
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

    lines
}
pub fn get_message_raw_content(msg: &HistoryMessage) -> String {
    match msg {
        HistoryMessage::User(blocks) => extract_text_from_blocks(blocks),
        HistoryMessage::Assistant {
            content, thinking, ..
        } => {
            let mut result = String::new();
            if let Some(thinking) = thinking {
                result.push_str("<thinking>\n");
                result.push_str(thinking);
                result.push_str("\n</thinking>\n\n");
            }
            result.push_str(content);
            result
        }
        HistoryMessage::Tool {
            tool_name,
            arguments,
            output,
            error,
            ..
        } => {
            let mut result = format!("Tool: {tool_name}\n");
            if let Some(args) = arguments {
                result.push_str("Arguments: ");
                result.push_str(args);
                result.push('\n');
            }
            if let Some(err) = error {
                result.push_str("Error: ");
                result.push_str(err);
            } else if let Some(out) = output {
                result.push_str("Output: ");
                result.push_str(out);
            }
            result
        }
        HistoryMessage::Error(error) => error.clone(),
        HistoryMessage::Notice(text) => text.clone(),
    }
}

/// Convert a message to pretty JSON
pub fn get_message_pretty_json(msg: &HistoryMessage) -> String {
    // Create a serializable representation
    #[derive(serde::Serialize, Default)]
    struct SerializableMessage {
        role: String,
        content: Option<String>,
        thinking: Option<String>,
        tool_name: Option<String>,
        tool_arguments: Option<String>,
        tool_output: Option<String>,
        tool_error: Option<String>,
        error: Option<String>,
    }

    let serializable = match msg {
        HistoryMessage::User(blocks) => SerializableMessage {
            role: "user".to_string(),
            content: Some(extract_text_from_blocks(blocks)),
            ..Default::default()
        },
        HistoryMessage::Assistant {
            content, thinking, ..
        } => SerializableMessage {
            role: "assistant".to_string(),
            content: Some(content.clone()),
            thinking: thinking.clone(),
            ..Default::default()
        },
        HistoryMessage::Tool {
            tool_name,
            arguments,
            output,
            error,
            ..
        } => SerializableMessage {
            role: "tool".to_string(),
            tool_name: Some(tool_name.clone()),
            tool_arguments: arguments.clone(),
            tool_output: output.clone(),
            tool_error: error.clone(),
            ..Default::default()
        },
        HistoryMessage::Error(error) => SerializableMessage {
            role: "error".to_string(),
            error: Some(error.clone()),
            ..Default::default()
        },
        HistoryMessage::Notice(text) => SerializableMessage {
            role: "notice".to_string(),
            content: Some(text.clone()),
            ..Default::default()
        },
    };

    serde_json::to_string_pretty(&serializable)
        .unwrap_or_else(|e| format!("{{\"error\": \"Failed to serialize: {e}\"}}"))
}

pub fn to_camel_case(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // If already starts with uppercase, assume it's already CamelCase
    if s.starts_with(|c: char| c.is_uppercase()) {
        return s.to_string();
    }

    // Convert first char to uppercase, keep rest as-is
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

pub fn tool_icon(tool_name: &str) -> &'static str {
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
        SLEEP_TOOL_NAME => "󰒲 ",
        // Task tools
        TASK_CREATE_TOOL_NAME
        | TASK_GET_TOOL_NAME
        | TASK_LIST_TOOL_NAME
        | TASK_UPDATE_TOOL_NAME
        | TODO_TOOL_NAME => " ",
        ASK_USER_TOOL_NAME => " ",
        _ => " ",
    }
}

/// Sanitize text for single-line display by replacing newlines/tabs with spaces.
pub fn sanitize_single_line(s: &str) -> String {
    s.replace(['\n', '\r', '\t'], " ")
}

/// Extract a concise description from tool arguments for the title
/// e.g., Read "src/main.rs", Edit "crates/tui/src/lib.rs"
/// Results are truncated to 100 characters (Unicode-safe).
pub fn extract_tool_target(tool_name: &str, args: Option<&str>) -> Option<String> {
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
        SUBAGENT_TOOL_NAME => value["description"].as_str().map(f),
        _ => None,
    };

    target.map(|t| truncate_by_chars(&sanitize_single_line(&t), MAX_LEN))
}
