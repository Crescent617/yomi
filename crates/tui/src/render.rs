use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::fold::FoldManager;
use crate::markdown::MarkdownRenderer;
use crate::theme::{chars, colors, Styles};

/// Render a user message block with distinctive styling
pub fn render_user(lines: &mut Vec<Line>, content: &str) {
    // Add spacing before message
    lines.push(Line::from(""));

    // Render each line with user accent bar on the left
    for (i, line) in content.lines().enumerate() {
        let prefix = if i == 0 {
            // First line: show user indicator with corner
            Span::styled(
                format!("{} ", chars::INPUT_PROMPT),
                Styles::user_header(),
            )
        } else {
            // Subsequent lines: aligned continuation
            Span::styled(
                format!("{} ", chars::INPUT_PROMPT_MULTI),
                Style::default().fg(colors::accent_user()),
            )
        };

        lines.push(Line::from(vec![
            prefix,
            Span::styled(line.to_string(), Styles::user_content()),
        ]));
    }
}

/// Render assistant message with clean styling
pub fn render_assistant(
    lines: &mut Vec<Line>,
    content: &str,
    thinking: Option<&str>,
    fold_manager: Option<&FoldManager>,
    msg_id: crate::model::MessageId,
) {
    let markdown = MarkdownRenderer::new();

    // Add spacing before message
    lines.push(Line::from(""));

    // Render thinking section if present (collapsible)
    if let Some(thinking) = thinking {
        let is_expanded = fold_manager
            .map(|fm| fm.is_expanded(msg_id))
            .unwrap_or(false);

        let indicator = if is_expanded {
            chars::FOLD_EXPANDED
        } else {
            chars::FOLD_COLLAPSED
        };

        let tokens = thinking.len() / 4;
        let header = format!("{} Thinking ({} tokens)", indicator, tokens);

        lines.push(Line::from(vec![Span::styled(
            header,
            Styles::thinking_header(),
        )]));

        if is_expanded {
            // Render thinking content with indentation
            for line in thinking.lines() {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(line.to_string(), Styles::thinking_content()),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    // Render main content
    let md_lines = markdown.render(content);
    lines.extend(md_lines);
}

/// Render tool call with elegant expandable design
pub fn render_tool(
    lines: &mut Vec<Line>,
    tool_name: &str,
    tool_input: &str,
    tool_output: &str,
    is_expanded: bool,
) {
    lines.push(Line::from(""));

    let indicator = if is_expanded {
        chars::FOLD_EXPANDED
    } else {
        chars::FOLD_COLLAPSED
    };

    // Tool header line
    let header = format!("{} Tool: {}", indicator, tool_name);
    lines.push(Line::from(vec![Span::styled(header, Styles::tool_header())]));

    if !tool_input.is_empty() && !is_expanded {
        // Show truncated input when collapsed
        let truncated: String = tool_input
            .chars()
            .take(40)
            .collect::<String>()
            .lines()
            .next()
            .unwrap_or("")
            .to_string();
        let ellipsis = if tool_input.len() > 40 { "..." } else { "" };
        lines.push(Line::from(vec![Span::styled(
            format!("  {}{}", truncated, ellipsis),
            Style::default().fg(colors::text_muted()),
        )]));
    }

    if is_expanded {
        // Show input section
        if !tool_input.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "  Input:",
                Style::default()
                    .fg(colors::text_secondary())
                    .add_modifier(Modifier::BOLD),
            )]));
            for line in tool_input.lines() {
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(line.to_string(), Style::default().fg(colors::text_secondary())),
                ]));
            }
        }

        // Show output section
        lines.push(Line::from(vec![Span::styled(
            "  Output:",
            Style::default()
                .fg(colors::text_secondary())
                .add_modifier(Modifier::BOLD),
        )]));
        for line in tool_output.lines() {
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(line.to_string(), Style::default().fg(colors::text_secondary())),
            ]));
        }
    }
}

/// Render system message with subtle styling
pub fn render_system(lines: &mut Vec<Line>, content: &str) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        content.to_string(),
        Styles::system(),
    )]));
}

/// Render streaming indicator
pub fn render_streaming_indicator(lines: &mut Vec<Line>, frame: usize) {
    let spinner = crate::theme::spinner_char(frame);
    lines.push(Line::from(vec![Span::styled(
        format!("{}", spinner),
        Styles::spinner(),
    )]));
}

/// Render input area with elegant prompt
pub fn render_input(
    input_lines: &[String],
    _cursor_line: usize,
    _cursor_col: usize,
) -> Vec<Line<'_>> {
    let mut lines = Vec::new();

    for (i, line) in input_lines.iter().enumerate() {
        let prefix = if i == 0 {
            // First line: main prompt
            Span::styled(
                format!("{} ", chars::INPUT_PROMPT),
                Styles::input_prompt(),
            )
        } else {
            // Continuation lines: aligned bar
            Span::styled(
                format!("{} ", chars::INPUT_PROMPT_MULTI),
                Style::default().fg(colors::accent_user()),
            )
        };

        lines.push(Line::from(vec![
            prefix,
            Span::styled(line.clone(), Styles::input_text()),
        ]));
    }

    // If empty, show placeholder
    if input_lines.is_empty() || (input_lines.len() == 1 && input_lines[0].is_empty()) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", chars::INPUT_PROMPT),
                Styles::input_prompt(),
            ),
            Span::styled("Type your message...", Styles::placeholder()),
        ]));
    }

    lines
}

/// Render a divider line
pub fn render_divider(lines: &mut Vec<Line>) {
    lines.push(Line::from(Span::styled(
        chars::CODE_HORIZONTAL.repeat(40),
        Style::default().fg(colors::divider()),
    )));
}

/// Render welcome message
pub fn render_welcome(lines: &mut Vec<Line>) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Welcome to ", Style::default().fg(colors::text_secondary())),
        Span::styled("Nekoclaw", Style::default().fg(colors::accent_user()).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(vec![Span::styled(
        "Your AI coding assistant. Press Ctrl+C twice to exit.",
        Style::default().fg(colors::text_muted()),
    )]));
    lines.push(Line::from(""));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_user() {
        let mut lines = Vec::new();
        render_user(&mut lines, "hello world");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_render_system() {
        let mut lines = Vec::new();
        render_system(&mut lines, "System message");
        assert_eq!(lines.len(), 2); // Empty line + content
    }

    #[test]
    fn test_render_input() {
        let input = vec!["test".to_string()];
        let lines = render_input(&input, 0, 0);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_render_welcome() {
        let mut lines = Vec::new();
        render_welcome(&mut lines);
        assert!(!lines.is_empty());
    }
}
