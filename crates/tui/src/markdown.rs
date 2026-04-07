use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Options, Parser, Tag, TagEnd};
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
        // Enable GFM extensions: tables, task lists, strikethrough
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_STRIKETHROUGH;
        let parser = Parser::new_ext(content, options);

        let mut current_line: Vec<Span> = Vec::new();
        let mut current_style = Style::default().fg(colors::text_primary());
        let mut in_code_block = false;
        let mut list_stack: Vec<Option<u64>> = Vec::new();
        let mut code_language: Option<String> = None;
        let mut table_header = false;
        let mut table_cell_count = 0;
        // Store table rows as (is_header, cells) tuples
        let mut table_rows: Vec<(bool, Vec<String>)> = Vec::new();
        let mut table_cell_widths: Vec<usize> = Vec::new();
        let mut current_row_cells: Vec<String> = Vec::new();

        for event in parser {
            match event {
                MdEvent::Start(tag) => {
                    match tag {
                        Tag::Strong => {
                            current_style = current_style.add_modifier(Modifier::BOLD);
                        }
                        Tag::Strikethrough => {
                            current_style = current_style.add_modifier(Modifier::CROSSED_OUT);
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
                        Tag::Table(_) => {
                            table_header = true;
                            table_cell_count = 0;
                            table_rows = Vec::new();
                            table_cell_widths = Vec::new();
                            current_row_cells = Vec::new();
                        }
                        Tag::TableHead => {
                            table_header = true;
                            table_cell_count = 0;
                        }
                        Tag::TableRow => {
                            current_row_cells = Vec::new();
                            table_cell_count = 0;
                        }
                        Tag::TableCell => {}
                        _ => {}
                    }
                }
                MdEvent::End(tag_end) => {
                    match tag_end {
                        TagEnd::Strong => {
                            current_style = current_style.remove_modifier(Modifier::BOLD);
                        }
                        TagEnd::Strikethrough => {
                            current_style = current_style.remove_modifier(Modifier::CROSSED_OUT);
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
                        TagEnd::TableCell => {
                            // Accumulate cell content
                            let cell_content: String = current_line
                                .iter()
                                .map(|s| s.content.clone())
                                .collect();
                            current_row_cells.push(cell_content);
                            current_line = Vec::new();
                            table_cell_count += 1;

                            // Track max width for this column
                            let cell_text: String = current_row_cells.last().unwrap().clone();
                            let width = unicode_width::UnicodeWidthStr::width(cell_text.trim());
                            if table_cell_widths.len() < table_cell_count {
                                table_cell_widths.push(width);
                            } else {
                                table_cell_widths[table_cell_count - 1] =
                                    table_cell_widths[table_cell_count - 1].max(width);
                            }
                        }
                        TagEnd::TableRow => {
                            // Store row for later rendering when we know column widths
                            if !current_row_cells.is_empty() {
                                table_rows.push((table_header, current_row_cells.clone()));
                            }
                            current_row_cells = Vec::new();
                        }
                        TagEnd::TableHead => {
                            table_header = false;
                        }
                        TagEnd::Table => {
                            // Render the table
                            if !table_rows.is_empty() {
                                // Add spacing before table
                                lines.push(Line::from(""));

                                // Ensure minimum column widths
                                for width in &mut table_cell_widths {
                                    *width = (*width).max(3); // At least 3 chars wide
                                }

                                // Render each row
                                for (is_header, row_cells) in &table_rows {
                                    let mut row_line = Vec::new();
                                    row_line.push(Span::styled(
                                        "│ ",
                                        Style::default().fg(colors::border()),
                                    ));

                                    for (i, cell) in row_cells.iter().enumerate() {
                                        let width = table_cell_widths.get(i).copied().unwrap_or(10);
                                        let trimmed = cell.trim();
                                        let padded = format!("{:width$}", trimmed, width = width);

                                        let style = if *is_header {
                                            Style::default()
                                                .fg(colors::accent_user())
                                                .add_modifier(Modifier::BOLD)
                                        } else {
                                            Style::default().fg(colors::text_primary())
                                        };
                                        row_line.push(Span::styled(padded, style));

                                        if i < row_cells.len().saturating_sub(1) {
                                            row_line.push(Span::styled(
                                                " │ ",
                                                Style::default().fg(colors::border()),
                                            ));
                                        }
                                    }

                                    row_line.push(Span::styled(
                                        " │",
                                        Style::default().fg(colors::border()),
                                    ));
                                    lines.push(Line::from(row_line));

                                    // Add separator line after header
                                    if *is_header {
                                        let mut sep = String::from("├─");
                                        for (i, width) in table_cell_widths.iter().enumerate() {
                                            sep.push_str(&"─".repeat(*width));
                                            if i < table_cell_widths.len().saturating_sub(1) {
                                                sep.push_str("─┼─");
                                            }
                                        }
                                        sep.push_str("─┤");
                                        lines.push(Line::from(Span::styled(
                                            sep,
                                            Style::default().fg(colors::border()),
                                        )));
                                    }
                                }

                                lines.push(Line::from(""));
                            }

                            table_header = false;
                            table_cell_count = 0;
                            table_rows = Vec::new();
                            table_cell_widths = Vec::new();
                            current_row_cells = Vec::new();
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
                MdEvent::TaskListMarker(checked) => {
                    // Task list checkbox
                    let checkbox = if checked { "[x]" } else { "[ ]" };
                    current_line.push(Span::styled(
                        format!("{checkbox} "),
                        Style::default().fg(if checked { colors::accent_user() } else { colors::text_secondary() }),
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
