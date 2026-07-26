//! Message rendering: pure functions that convert `HistoryMessage` / streaming content
//! into ratatui Lines.
//!
//! This module is intentionally stateless. All rendering decisions are passed as parameters.

use std::sync::Arc;

use tuirealm::ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::components::chat_view::{HistoryMessage, SubagentState, ToolStatus};
use crate::markdown_stream::StreamingMarkdownRenderer;
use crate::theme::{chars, colors};
use crate::utils::text::{humanize_tool_name, preprocess, truncate_by_chars, truncate_by_width};

use kernel::types::{ContentBlock, ToolOutputBlock};
use kernel::utils::tokens;
use unicode_width::UnicodeWidthStr;

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

/// Max diff lines shown for a folded Edit tool; browse-mode expand shows all.
const FOLDED_EDIT_DIFF_LINES: usize = 10;

/// Push one styled diff line (`add`/`del`/`context`) for the Edit tool view.
fn push_edit_diff_line(lines: &mut Vec<Arc<Line<'static>>>, ty: &str, text: &str) {
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

/// Parse Edit tool arguments into a line diff of `old_str` → `new_str`.
fn edit_args_diff(arguments: Option<&str>) -> Option<Vec<(&'static str, String)>> {
    let parsed: serde_json::Value = serde_json::from_str(arguments?).ok()?;
    let old_str = parsed["old_str"].as_str()?;
    let new_str = parsed["new_str"].as_str()?;
    Some(diff_lines(old_str, new_str))
}

/// Folded Edit preview: render at most `FOLDED_EDIT_DIFF_LINES` diff lines
/// with an overflow hint.
fn render_edit_diff_peek(lines: &mut Vec<Arc<Line<'static>>>, diff: &[(&str, String)]) {
    for (ty, text) in diff.iter().take(FOLDED_EDIT_DIFF_LINES) {
        push_edit_diff_line(lines, ty, text);
    }
    let hidden = diff.len().saturating_sub(FOLDED_EDIT_DIFF_LINES);
    if hidden > 0 {
        lines.push(Arc::new(Line::from(vec![
            Span::styled(
                chars::MSG_INDENT2_GUIDE,
                Style::default().fg(colors::text_secondary()),
            ),
            Span::styled(
                format!("  +{hidden} more lines (Ctrl+E in browse mode to expand)"),
                Style::default()
                    .fg(colors::text_secondary())
                    .add_modifier(Modifier::ITALIC),
            ),
        ])));
    }
}

#[allow(clippy::cast_precision_loss)]
pub fn render_message(msg: &HistoryMessage, width: usize) -> Vec<Arc<Line<'static>>> {
    match msg {
        HistoryMessage::User(blocks) => render_user(blocks, width),
        HistoryMessage::Steer(blocks) => render_steer(blocks),
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
            width,
        ),
        HistoryMessage::Tool {
            tool_name,
            tool_id: _,
            status,
            output,
            error,
            folded,
            arguments,
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
            *elapsed_ms,
            content_blocks,
            subagent.as_ref(),
            width,
        ),
        HistoryMessage::Error(error) => render_error(error),
        HistoryMessage::Notice(text) => render_notice(text),
    }
}

/// Pad a line with background-colored spaces so its background fills the
/// full row width (user message bubble effect). A line padded to exactly
/// `width` still occupies a single visual row after wrapping.
fn push_bg_padding(spans: &mut Vec<Span<'static>>, width: usize, bg: Color) {
    let used: usize = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if used < width {
        spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(bg),
        ));
    }
}

fn render_user(content_blocks: &[ContentBlock], width: usize) -> Vec<Arc<Line<'static>>> {
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
                    let mut spans = vec![
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
                    ];
                    push_bg_padding(&mut spans, width, user_bg);
                    lines.push(Arc::new(Line::from(spans)));
                    line_idx += 1;
                }
            }
            ContentBlock::ImageUrl { .. } => {
                let prefix = if line_idx == 0 {
                    chars::INPUT_PROMPT
                } else {
                    chars::INPUT_PROMPT_MULTI
                };
                let mut spans = vec![
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
                ];
                push_bg_padding(&mut spans, width, user_bg);
                lines.push(Arc::new(Line::from(spans)));
                line_idx += 1;
            }
            _ => {}
        }
    }
    lines
}

fn render_steer(content_blocks: &[ContentBlock]) -> Vec<Arc<Line<'static>>> {
    let mut lines = Vec::new();
    let mut first = true;
    for block in content_blocks {
        let ContentBlock::Text { text } = block else {
            continue;
        };
        for line in text.lines() {
            let prefix = if first { " " } else { "  " };
            lines.push(Arc::new(Line::from(vec![
                Span::styled(
                    prefix,
                    Style::default()
                        .fg(colors::accent_user())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    preprocess(line),
                    Style::default().fg(colors::text_primary()),
                ),
            ])));
            first = false;
        }
    }
    lines
}

fn render_assistant(
    content: &str,
    thinking: Option<&str>,
    thinking_folded: bool,
    thinking_elapsed_ms: Option<u64>,
    width: usize,
) -> Vec<Arc<Line<'static>>> {
    let mut lines = Vec::new();

    // Render thinking summary (folded) or detail (expanded)
    let thinking_lines = thinking
        .as_ref()
        .map(|t| render_thinking_lines(t, thinking_folded, thinking_elapsed_ms, false, width))
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
        md_renderer.set_width(width);
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
    let kind = tool_kind(tool_name);
    // Parse the Edit diff once: used for header stats, folded peek and the
    // expanded diff view.
    let edit_diff = if kind == ToolKind::Edit {
        edit_args_diff(arguments)
    } else {
        None
    };

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

    let summary = tool_header_summary(tool_name, arguments);

    // Tool name with status color
    let tool_part = format!("{icon}{}{time_str}", summary.label);
    let mut header_spans = vec![Span::styled(
        tool_part,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )];

    // Target/args with text_primary color (no bold)
    if let Some(target) = summary.target {
        header_spans.push(Span::styled(
            format!(" {target}"),
            Style::default().fg(colors::text_primary()),
        ));
    } else if let Some(peek) = peek_args {
        // Fallback to peek_args if we couldn't extract a target
        header_spans.push(Span::styled(
            format!(" {peek}"),
            Style::default().fg(colors::text_primary()),
        ));
    }

    if let Some(metadata) = summary.metadata {
        header_spans.push(Span::styled(
            format!(" {metadata}"),
            Style::default().fg(colors::text_secondary()),
        ));
    }

    // Edit diff stats: "+3 −1" appended to the header (skipped for no-op diffs).
    if let Some(ref diff) = edit_diff {
        let adds = diff.iter().filter(|(ty, _)| *ty == "add").count();
        let dels = diff.iter().filter(|(ty, _)| *ty == "del").count();
        if adds > 0 || dels > 0 {
            header_spans.push(Span::styled(
                format!(" +{adds}"),
                Style::default().fg(colors::accent_success()),
            ));
            header_spans.push(Span::styled(
                format!(" −{dels}"),
                Style::default().fg(colors::accent_error()),
            ));
        }
    }

    lines.push(Arc::new(Line::from(header_spans)));

    // Output peek in folded mode: show the first two real (non-empty) output
    // lines instead of whitespace-collapsed text, preserving line structure.
    if folded {
        // Edit tool: preview the pending change as a compact diff directly
        // (capped at FOLDED_EDIT_DIFF_LINES; browse-mode Ctrl+E shows all).
        let edit_diff_shown = edit_diff.is_some();
        if let Some(ref diff) = edit_diff {
            render_edit_diff_peek(&mut lines, diff);
        }

        // The diff already conveys a successful edit, so keep the peek only
        // for errors or when no diff could be rendered.
        if error.is_some() || !edit_diff_shown {
            if let Some(out) = error.or(output) {
                let non_empty: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
                for (i, out_line) in non_empty.iter().take(2).enumerate() {
                    let prefix = if i == 0 { " ⎿ " } else { "   " };
                    let hidden = non_empty.len().saturating_sub(2);
                    let more = if i == 1 && hidden > 0 {
                        format!(" … +{hidden}")
                    } else {
                        String::new()
                    };
                    let avail = width.saturating_sub(3 + UnicodeWidthStr::width(more.as_str()));
                    let mut spans = vec![
                        Span::styled(prefix, Style::default().fg(colors::text_secondary())),
                        Span::styled(
                            truncate_by_width(out_line.trim(), avail, "..."),
                            Style::default().fg(colors::text_secondary()),
                        ),
                    ];
                    if !more.is_empty() {
                        spans.push(Span::styled(
                            more,
                            Style::default()
                                .fg(colors::text_secondary())
                                .add_modifier(Modifier::ITALIC),
                        ));
                    }
                    lines.push(Arc::new(Line::from(spans)));
                }
            }
        }
    } else {
        // Show tool arguments if available
        if let Some(args) = arguments {
            if !args.is_empty() {
                if kind == ToolKind::Edit {
                    // Special diff view for edit tool (full diff)
                    let mut diff_rendered = false;
                    if let Some(ref diff) = edit_diff {
                        diff_rendered = true;
                        for (ty, text) in diff {
                            push_edit_diff_line(&mut lines, ty, text);
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
                } else if kind == ToolKind::PostMessage {
                    let mut rendered = false;
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(args) {
                        if let (Some(agent_id), Some(title), Some(content)) = (
                            parsed["agent_id"].as_str(),
                            parsed["title"].as_str(),
                            parsed["content"].as_str(),
                        ) {
                            rendered = true;
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(
                                    chars::MSG_INDENT_GUIDE,
                                    Style::default().fg(colors::text_secondary()),
                                ),
                                Span::styled("To ", Style::default().fg(colors::text_secondary())),
                                Span::styled(
                                    sanitize_single_line(&preprocess(agent_id)),
                                    Style::default().fg(colors::accent_system()),
                                ),
                            ])));
                            lines.push(Arc::new(Line::from(vec![
                                Span::styled(
                                    chars::MSG_INDENT_GUIDE,
                                    Style::default().fg(colors::text_secondary()),
                                ),
                                Span::styled(
                                    sanitize_single_line(&preprocess(title)),
                                    Style::default()
                                        .fg(colors::text_primary())
                                        .add_modifier(Modifier::BOLD),
                                ),
                            ])));
                            for line in content.lines() {
                                lines.push(Arc::new(Line::from(vec![
                                    Span::styled(
                                        chars::MSG_INDENT_GUIDE,
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                    Span::styled(
                                        preprocess(line),
                                        Style::default().fg(colors::text_primary()),
                                    ),
                                ])));
                            }
                        }
                    }
                    if !rendered {
                        render_raw_tool_arguments(&mut lines, args);
                    }
                } else {
                    render_raw_tool_arguments(&mut lines, args);
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
                        format!("{} {t}", tool_label(tool_name))
                    } else {
                        tool_label(tool_name)
                    }
                }
                kernel::event::Event::Tool(kernel::event::ToolEvent::End {
                    tool_name,
                    is_error,
                    ..
                }) => {
                    if *is_error {
                        format!(" {}", tool_label(tool_name))
                    } else {
                        format!(" {}", tool_label(tool_name))
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
        "󰔟 Queued (Enter again: steer · ↑/Esc: edit · auto-send at stream end)",
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

/// Number of trailing thinking lines shown as preview when folded.
const THINKING_PREVIEW_LINES: usize = 4;

/// Render thinking content.
///
/// - Folded: up to [`THINKING_PREVIEW_LINES`] preview rows showing the last
///   non-empty lines (empty lines are filtered out), each truncated to a
///   single visual row. Token/elapsed stats are appended inline to the last
///   preview row once completed; while streaming no stats are shown (the
///   `InfoBar` covers live status). Row count is identical between streaming
///   and completed states (except whitespace-only thinking), so the block
///   height never jumps on flush.
/// - Expanded: full content, no stats.
///
/// Returns empty vec if thinking is empty.
#[allow(clippy::cast_precision_loss)]
pub fn render_thinking_lines(
    thinking: &str,
    is_folded: bool,
    elapsed_ms: Option<u64>,
    is_streaming: bool,
    width: usize,
) -> Vec<Arc<Line<'static>>> {
    if thinking.is_empty() {
        return Vec::new();
    }

    let guide = || {
        Span::styled(
            chars::MSG_INDENT_GUIDE,
            Style::default().fg(colors::text_secondary()),
        )
    };
    // Italic marks thinking as internal monologue and visually separates it
    // from tool output above/below, which is dim but never italic.
    let content_style = Style::default()
        .fg(colors::text_secondary())
        .add_modifier(Modifier::ITALIC);

    let mut lines = Vec::new();

    if is_folded {
        // "│ " guide occupies 2 display columns.
        let avail = width.saturating_sub(2);

        // Stats are appended inline to the last preview row (no dedicated
        // row). Truncate stats itself so the row never exceeds `width`.
        let stats = if is_streaming {
            None
        } else {
            let token_count = tokens::estimate_tokens(thinking);
            let elapsed_str = elapsed_ms
                .map(|ms| format!(" · {:.1}s", ms as f64 / 1000.0))
                .unwrap_or_default();
            let raw = format!("··· {token_count} tokens{elapsed_str}");
            Some(truncate_by_width(&raw, avail.saturating_sub(2), "…"))
        };
        let stats_reserve = stats
            .as_ref()
            .map_or(0, |s| UnicodeWidthStr::width(s.as_str()) + 2);

        // Preview shows the last non-empty lines; empty lines are filtered.
        let mut tail: Vec<&str> = thinking
            .lines()
            .rev()
            .filter(|l| !l.trim().is_empty())
            .take(THINKING_PREVIEW_LINES)
            .collect();
        tail.reverse();

        let last_idx = tail.len().saturating_sub(1);
        for (i, line) in tail.iter().enumerate() {
            let is_last = i == last_idx;
            let reserve = if is_last { stats_reserve } else { 0 };
            let mut spans = vec![
                guide(),
                Span::styled(
                    truncate_by_width(&preprocess(line), avail.saturating_sub(reserve), "..."),
                    content_style,
                ),
            ];
            if is_last {
                if let Some(ref s) = stats {
                    spans.push(Span::styled(format!("  {s}"), content_style));
                }
            }
            lines.push(Arc::new(Line::from(spans)));
        }

        // Whitespace-only thinking: stats (when completed) still get a row.
        if tail.is_empty() {
            if let Some(s) = stats {
                lines.push(Arc::new(Line::from(Span::styled(s, content_style))));
            }
        }
    } else {
        for line in thinking.lines() {
            lines.push(Arc::new(Line::from(vec![
                guide(),
                Span::styled(preprocess(line), content_style),
            ])));
        }
    }

    lines
}
pub fn get_message_raw_content(msg: &HistoryMessage) -> String {
    match msg {
        HistoryMessage::User(blocks) | HistoryMessage::Steer(blocks) => {
            extract_text_from_blocks(blocks)
        }
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
        HistoryMessage::Steer(blocks) => SerializableMessage {
            role: "steer".to_string(),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolKind {
    Read,
    Write,
    Edit,
    Shell,
    Glob,
    Grep,
    WebFetch,
    WebSearch,
    Skill,
    Agent,
    PostMessage,
    AskUser,
    Todo,
    Reminder,
    Sleep,
    UpdateGoal,
    SendMessage,
    Cron,
    TaskCreate,
    TaskGet,
    TaskList,
    TaskUpdate,
    Other,
}

#[derive(Debug, PartialEq, Eq)]
struct ToolHeaderSummary {
    label: String,
    target: Option<String>,
    metadata: Option<String>,
}

fn normalized_tool_name(tool_name: &str) -> String {
    tool_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn tool_kind(tool_name: &str) -> ToolKind {
    match normalized_tool_name(tool_name).as_str() {
        "read" | "readfile" => ToolKind::Read,
        "write" | "writefile" => ToolKind::Write,
        "edit" | "editfile" => ToolKind::Edit,
        "shell" | "bash" | "command" => ToolKind::Shell,
        "glob" | "globsearch" => ToolKind::Glob,
        "grep" | "grepsearch" => ToolKind::Grep,
        "webfetch" => ToolKind::WebFetch,
        "websearch" => ToolKind::WebSearch,
        "skill" => ToolKind::Skill,
        "agent" | "subagent" => ToolKind::Agent,
        "postmessage" => ToolKind::PostMessage,
        "askuser" | "ask" => ToolKind::AskUser,
        "todo" | "task" => ToolKind::Todo,
        "reminder" => ToolKind::Reminder,
        "sleep" => ToolKind::Sleep,
        "updategoal" => ToolKind::UpdateGoal,
        "sendmessage" | "message" => ToolKind::SendMessage,
        "cron" => ToolKind::Cron,
        "taskcreate" => ToolKind::TaskCreate,
        "taskget" => ToolKind::TaskGet,
        "tasklist" => ToolKind::TaskList,
        "taskupdate" => ToolKind::TaskUpdate,
        _ => ToolKind::Other,
    }
}

fn tool_header_summary(tool_name: &str, args: Option<&str>) -> ToolHeaderSummary {
    let kind = tool_kind(tool_name);
    let label = tool_label(tool_name);
    let value = args.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let text = |key: &str| value.as_ref()?.get(key)?.as_str();
    let compact = |s: &str| truncate_by_chars(&sanitize_single_line(s), 100);
    let target = match kind {
        ToolKind::Read | ToolKind::Edit => text("path").map(compact),
        ToolKind::Write => text("file_path").map(compact),
        ToolKind::Shell => text("command").map(compact),
        ToolKind::Glob | ToolKind::Grep => text("pattern").map(compact),
        ToolKind::WebFetch => text("url").map(compact),
        ToolKind::WebSearch => text("query").map(compact),
        ToolKind::Skill => text("name").or_else(|| text("path")).map(compact),
        ToolKind::Agent => text("description").map(compact),
        ToolKind::PostMessage => text("agent_id").map(compact),
        ToolKind::AskUser => value
            .as_ref()
            .and_then(|v| v["questions"].as_array())
            .and_then(|questions| questions.first())
            .and_then(|question| question["question"].as_str())
            .map(compact),
        ToolKind::Todo | ToolKind::Cron => text("action").map(compact),
        ToolKind::Reminder => text("message").map(compact),
        ToolKind::Sleep => value
            .as_ref()
            .and_then(|v| v["seconds"].as_u64())
            .map(|seconds| format!("{seconds}s")),
        ToolKind::UpdateGoal => text("status").map(compact),
        ToolKind::SendMessage => text("content")
            .or_else(|| {
                value
                    .as_ref()
                    .and_then(|v| v["files"].as_array())
                    .and_then(|files| files.first())
                    .and_then(serde_json::Value::as_str)
            })
            .map(compact),
        ToolKind::TaskCreate => text("subject").map(compact),
        ToolKind::TaskGet | ToolKind::TaskUpdate => text("taskId").map(compact),
        ToolKind::TaskList | ToolKind::Other => None,
    };

    let mut metadata = Vec::new();
    match kind {
        ToolKind::Read => {
            let offset = value.as_ref().and_then(|v| v["offset"].as_u64());
            let limit = value.as_ref().and_then(|v| v["limit"].as_u64());
            if offset.is_some() || limit.is_some() {
                let start = offset.unwrap_or(1);
                metadata.push(match limit {
                    Some(limit) => format!(
                        "lines {start}-{}",
                        start.saturating_add(limit).saturating_sub(1)
                    ),
                    None => format!("from line {start}"),
                });
            }
        }
        ToolKind::Write if text("mode") == Some("append") => {
            metadata.push("append".to_string());
        }
        ToolKind::Edit if value.as_ref().and_then(|v| v["replace_all"].as_bool()) == Some(true) => {
            metadata.push("replace all".to_string());
        }
        ToolKind::Shell => {
            let background = value
                .as_ref()
                .and_then(|v| v["background"].as_bool())
                .unwrap_or(false);
            if background {
                metadata.push("async".to_string());
            }
            if let Some(timeout) = value.as_ref().and_then(|v| v["timeout"].as_u64()) {
                if background || timeout != 60 {
                    metadata.push(format!("timeout {timeout}s"));
                }
            }
        }
        ToolKind::Glob => {
            if let Some(path) = text("path") {
                metadata.push(compact(path));
            }
        }
        ToolKind::Grep => {
            let mode = value
                .as_ref()
                .and_then(|v| v["output_mode"].as_str())
                .unwrap_or("filename");
            if mode != "filename" {
                metadata.push(mode.to_string());
            }
            for key in ["path", "glob", "type"] {
                if let Some(item) = text(key) {
                    metadata.push(compact(item));
                }
            }
            if let Some(context) = value
                .as_ref()
                .and_then(|v| v["context"].as_u64().or_else(|| v["-C"].as_u64()))
            {
                metadata.push(format!("context {context}"));
            }
        }
        ToolKind::WebSearch => {
            if let Some(count) = value.as_ref().and_then(|v| v["num_results"].as_u64()) {
                metadata.push(format!("{count} results"));
            }
        }
        ToolKind::Agent
            if value
                .as_ref()
                .and_then(|v| v["wait_for_completion"].as_bool())
                == Some(false) =>
        {
            metadata.push("async".to_string());
        }
        ToolKind::PostMessage => {
            if let Some(title) = text("title") {
                metadata.push(compact(title));
            }
        }
        ToolKind::AskUser => {
            if let Some(questions) = value.as_ref().and_then(|v| v["questions"].as_array()) {
                if let Some(header) = questions
                    .first()
                    .and_then(|question| question["header"].as_str())
                {
                    metadata.push(compact(header));
                }
                if questions.len() > 1 {
                    metadata.push(format!("{} questions", questions.len()));
                }
            }
        }
        ToolKind::Todo => {
            if let Some(items) = value.as_ref().and_then(|v| v["todos"].as_array()) {
                metadata.push(format!("{} items", items.len()));
            }
        }
        ToolKind::Cron => match text("action") {
            Some("create") => {
                if let Some(name) = text("name") {
                    metadata.push(compact(name));
                }
                if let Some(schedule) = text("schedule") {
                    metadata.push(compact(schedule));
                }
                if text("type") == Some("shell") {
                    metadata.push("shell".to_string());
                }
                if let Some(max_runs) = value.as_ref().and_then(|v| v["max_runs"].as_u64()) {
                    metadata.push(format!("max {max_runs}"));
                }
            }
            Some("update") => {
                if let Some(id) = text("id") {
                    metadata.push(compact(id));
                }
                if let Some(status) = text("status") {
                    metadata.push(format!("→ {status}"));
                } else if let Some(schedule) = text("schedule") {
                    metadata.push(compact(schedule));
                }
            }
            Some("delete" | "trigger") => {
                if let Some(id) = text("id") {
                    metadata.push(compact(id));
                }
            }
            Some("list") => {
                if let Some(status) = text("status") {
                    metadata.push(status.to_string());
                }
            }
            _ => {}
        },
        ToolKind::Reminder => {
            if let Some(delay) = value.as_ref().and_then(|v| v["delay_seconds"].as_u64()) {
                metadata.push(format!("{delay}s"));
            }
        }
        ToolKind::SendMessage => {
            if let Some(files) = value.as_ref().and_then(|v| v["files"].as_array()) {
                if !files.is_empty() {
                    metadata.push(format!("{} files", files.len()));
                }
            }
        }
        ToolKind::TaskList
            if value.as_ref().and_then(|v| v["includeCompleted"].as_bool()) == Some(true) =>
        {
            metadata.push("include completed".to_string());
        }
        ToolKind::TaskUpdate => {
            for key in ["status", "subject"] {
                if let Some(item) = text(key) {
                    metadata.push(compact(item));
                }
            }
        }
        _ => {}
    }

    ToolHeaderSummary {
        label,
        target,
        metadata: (!metadata.is_empty()).then(|| metadata.join(" · ")),
    }
}

/// Extract the primary target from tool arguments for inline activity rendering.
pub fn extract_tool_target(tool_name: &str, args: Option<&str>) -> Option<String> {
    tool_header_summary(tool_name, args).target
}

/// Present-tense verb shown while a tool runs ("Editing foo.rs" instead of
/// "Calling Edit"). Tools without a dedicated verb fall back to "Calling".
pub fn tool_verb(tool_name: &str) -> &'static str {
    match tool_kind(tool_name) {
        ToolKind::Read => "Reading",
        ToolKind::Write => "Writing",
        ToolKind::Edit => "Editing",
        ToolKind::Shell => "Running",
        ToolKind::Glob | ToolKind::Grep | ToolKind::WebSearch => "Searching",
        ToolKind::WebFetch => "Fetching",
        ToolKind::Agent => "Delegating",
        ToolKind::Sleep => "Sleeping",
        _ => "Calling",
    }
}

fn tool_label(tool_name: &str) -> String {
    match tool_kind(tool_name) {
        ToolKind::Read => "Read",
        ToolKind::Write => "Write",
        ToolKind::Edit => "Edit",
        ToolKind::Shell => "Shell",
        ToolKind::Glob => "Glob",
        ToolKind::Grep => "Grep",
        ToolKind::WebFetch => "WebFetch",
        ToolKind::WebSearch => "WebSearch",
        ToolKind::Skill => "Skill",
        ToolKind::Agent => "Agent",
        ToolKind::PostMessage => "PostMessage",
        ToolKind::AskUser => "AskUser",
        ToolKind::Todo => "Todo",
        ToolKind::Reminder => "Reminder",
        ToolKind::Sleep => "Sleep",
        ToolKind::UpdateGoal => "UpdateGoal",
        ToolKind::SendMessage => "SendMessage",
        ToolKind::Cron => "Cron",
        ToolKind::TaskCreate => "TaskCreate",
        ToolKind::TaskGet => "TaskGet",
        ToolKind::TaskList => "TaskList",
        ToolKind::TaskUpdate => "TaskUpdate",
        ToolKind::Other => return humanize_tool_name(tool_name),
    }
    .to_string()
}

pub fn tool_icon(tool_name: &str) -> &'static str {
    match tool_kind(tool_name) {
        ToolKind::Agent => "󰚩 ",
        ToolKind::Read => " ",
        ToolKind::Write | ToolKind::Edit => " ",
        ToolKind::Shell => " ",
        ToolKind::Glob => "󰱼 ",
        ToolKind::Grep => "󰑑 ",
        ToolKind::Skill => "⚡",
        ToolKind::WebFetch => "󰖟 ",
        ToolKind::WebSearch => " ",
        ToolKind::PostMessage | ToolKind::SendMessage => "󰍩 ",
        ToolKind::Cron => "󰥔 ",
        ToolKind::Reminder => "󰀠 ",
        ToolKind::Sleep => "󰒲 ",
        ToolKind::TaskCreate
        | ToolKind::TaskGet
        | ToolKind::TaskList
        | ToolKind::TaskUpdate
        | ToolKind::Todo => " ",
        ToolKind::AskUser => " ",
        _ => " ",
    }
}

fn render_raw_tool_arguments(lines: &mut Vec<Arc<Line<'static>>>, args: &str) {
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

/// Sanitize text for single-line display by replacing newlines/tabs with spaces.
pub fn sanitize_single_line(s: &str) -> String {
    s.replace(['\n', '\r', '\t'], " ")
}

#[cfg(test)]
#[path = "message_renderer_test.rs"]
mod tests;
