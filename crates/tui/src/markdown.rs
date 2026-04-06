use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::{chars, colors, Styles};

/// Lightweight markdown renderer for streaming content
pub struct MarkdownRenderer;

impl MarkdownRenderer {
    pub const fn new() -> Self {
        Self
    }

    /// Render markdown content to ratatui Lines
    pub fn render(&self, content: &str) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let parser = Parser::new(content);

        let mut current_line: Vec<Span> = Vec::new();
        let mut current_style = Style::default().fg(colors::text_primary());
        let mut in_code_block = false;
        let mut list_stack: Vec<Option<u64>> = Vec::new();
        let mut code_language: Option<String> = None;

        for event in parser {
            match event {
                MdEvent::Start(tag) => {
                    match tag {
                        Tag::Strong => {
                            current_style = current_style.add_modifier(Modifier::BOLD);
                        }
                        Tag::Emphasis => {
                            current_style = current_style.add_modifier(Modifier::ITALIC);
                        }
                        Tag::CodeBlock(kind) => {
                            in_code_block = true;
                            // Flush current line before code block
                            if !current_line.is_empty() {
                                lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
                            // Extract language if available
                            if let CodeBlockKind::Fenced(lang) = kind {
                                code_language = Some(lang.to_string());
                            }
                        }
                        Tag::List(start_num) => {
                            list_stack.push(start_num);
                        }
                        Tag::Item => {
                            let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                            let prefix = match list_stack.last().copied().flatten() {
                                Some(num) => format!("{indent}{num}. "),
                                None => format!("{indent}{} ", chars::BULLET),
                            };
                            current_line.push(Span::styled(
                                prefix,
                                Style::default().fg(colors::accent_user()),
                            ));
                        }
                        Tag::Heading { level, .. } => {
                            if !current_line.is_empty() {
                                lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
                            // Add spacing before heading
                            lines.push(Line::from(""));
                            current_style = match level {
                                pulldown_cmark::HeadingLevel::H1 => {
                                    Style::default()
                                        .fg(colors::accent_user())
                                        .add_modifier(Modifier::BOLD)
                                }
                                pulldown_cmark::HeadingLevel::H2 => {
                                    Style::default()
                                        .fg(colors::text_primary())
                                        .add_modifier(Modifier::BOLD)
                                }
                                _ => Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::BOLD),
                            };
                        }
                        Tag::BlockQuote(_) => {
                            current_line.push(Span::styled(
                                format!("{} ", chars::USER_BAR),
                                Style::default().fg(colors::border()),
                            ));
                        }
                        _ => {}
                    }
                }
                MdEvent::End(tag_end) => {
                    match tag_end {
                        TagEnd::Strong => {
                            current_style = current_style.remove_modifier(Modifier::BOLD);
                        }
                        TagEnd::Emphasis => {
                            current_style = current_style.remove_modifier(Modifier::ITALIC);
                        }
                        TagEnd::CodeBlock => {
                            in_code_block = false;
                            if !current_line.is_empty() {
                                lines.push(Line::from(
                                    current_line
                                        .into_iter()
                                        .map(|s| {
                                            Span::styled(s.content, Style::default().fg(colors::code_fg()))
                                        })
                                        .collect::<Vec<_>>(),
                                ));
                                current_line = Vec::new();
                            }
                            // Close code block with border
                            lines.push(Line::from(Span::styled(
                                format!("{}{}", chars::CODE_BOTTOM_LEFT, chars::CODE_HORIZONTAL.repeat(40)),
                                Style::default().fg(colors::code_border()),
                            )));
                            code_language = None;
                        }
                        TagEnd::List(_) => {
                            list_stack.pop();
                        }
                        TagEnd::Heading(_) => {
                            if !current_line.is_empty() {
                                lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
                            // Add spacing after heading
                            lines.push(Line::from(""));
                            current_style = Style::default().fg(colors::text_primary());
                        }
                        TagEnd::Paragraph => {
                            if !current_line.is_empty() {
                                lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
                            lines.push(Line::from(""));
                        }
                        _ => {}
                    }
                }
                MdEvent::Text(text) => {
                    if in_code_block {
                        // Code block content - each line is separate
                        for line in text.lines() {
                            if current_line.is_empty() && code_language.is_some() {
                                // First line of code block - show language header
                                let lang = code_language.take().unwrap();
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        format!("{}{} ", chars::CODE_TOP_LEFT, chars::CODE_HORIZONTAL.repeat(2)),
                                        Style::default().fg(colors::code_border()),
                                    ),
                                    Span::styled(lang, Styles::code_lang()),
                                ]));
                            }
                            if !current_line.is_empty() {
                                lines.push(Line::from(
                                    current_line
                                        .into_iter()
                                        .map(|s| {
                                            Span::styled(s.content, Style::default().fg(colors::code_fg()))
                                        })
                                        .collect::<Vec<_>>(),
                                ));
                                current_line = Vec::new();
                            }
                            lines.push(Line::from(vec![
                                Span::styled(
                                    format!("{} ", chars::CODE_VERTICAL),
                                    Style::default().fg(colors::code_border()),
                                ),
                                Span::styled(
                                    line.to_string(),
                                    Style::default().fg(colors::code_fg()),
                                ),
                            ]));
                        }
                    } else {
                        current_line.push(Span::styled(text.to_string(), current_style));
                    }
                }
                MdEvent::Code(code) => {
                    // Inline code
                    current_line.push(Span::styled(
                        format!(" `{code}` "),
                        Styles::inline_code(),
                    ));
                }
                MdEvent::SoftBreak | MdEvent::HardBreak => {
                    if in_code_block {
                        if !current_line.is_empty() {
                            lines.push(Line::from(
                                current_line
                                    .into_iter()
                                    .map(|s| {
                                        Span::styled(s.content, Style::default().fg(colors::code_fg()))
                                    })
                                    .collect::<Vec<_>>(),
                            ));
                            current_line = Vec::new();
                        }
                    } else if !current_line.is_empty() {
                        lines.push(Line::from(current_line));
                        current_line = Vec::new();
                    }
                }
                MdEvent::Rule => {
                    lines.push(Line::from(Span::styled(
                        format!("{} ", chars::BULLET).repeat(20).trim_end().to_string(),
                        Style::default().fg(colors::divider()),
                    )));
                }
                _ => {}
            }
        }

        // Add remaining content
        if !current_line.is_empty() {
            if in_code_block {
                lines.push(Line::from(
                    current_line
                        .into_iter()
                        .map(|s| Span::styled(s.content, Style::default().fg(colors::code_fg())))
                        .collect::<Vec<_>>(),
                ));
            } else {
                lines.push(Line::from(current_line));
            }
        }

        // Remove trailing empty lines
        while lines.last().is_some_and(|l| l.to_string().trim().is_empty()) {
            lines.pop();
        }

        lines
    }
}

impl Default for MarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}
