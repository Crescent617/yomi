//! Streaming markdown renderer with delta updates
//!
//! This renderer is optimized for streaming content, tracking state
//! and only re-rendering when necessary.

use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Options, Parser, Tag, TagEnd};
use tuirealm::ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::{chars, colors, Styles};

/// Tracks the state of markdown parsing for incremental rendering
#[derive(Debug, Clone)]
struct ParseState {
    in_code_block: bool,
    code_language: Option<String>,
    list_stack: Vec<Option<u64>>,
    current_style: Style,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            in_code_block: false,
            code_language: None,
            list_stack: Vec::new(),
            current_style: Style::default().fg(colors::text_primary()),
        }
    }
}

/// Streaming markdown renderer that supports incremental updates
#[derive(Debug, Default)]
pub struct StreamingMarkdownRenderer {
    content: String,
    lines: Vec<Line<'static>>,
    state: ParseState,
    dirty: bool,
}

impl StreamingMarkdownRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append new text and re-render
    pub fn append(&mut self, text: &str) -> &[Line<'static>] {
        if text.is_empty() {
            return &self.lines;
        }

        self.content.push_str(text);
        self.dirty = true;
        self.render()
    }

    /// Set content and re-render
    pub fn set_content(&mut self, content: String) -> &[Line<'static>] {
        self.content = content;
        self.lines.clear();
        self.state = ParseState::default();
        self.dirty = true;
        self.render()
    }

    /// Get current content
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Get rendered lines (re-render if dirty)
    pub fn lines(&mut self) -> &[Line<'static>] {
        if self.dirty {
            self.render();
        }
        &self.lines
    }

    /// Force re-render
    fn render(&mut self) -> &[Line<'static>] {
        self.lines.clear();

        let options = Options::ENABLE_TABLES
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_STRIKETHROUGH;

        let parser = Parser::new_ext(&self.content, options);

        let mut current_line: Vec<Span> = Vec::new();
        let mut in_code_block = self.state.in_code_block;
        let mut code_language = self.state.code_language.clone();
        let mut list_stack: Vec<Option<u64>> = self.state.list_stack.clone();
        let mut current_style = self.state.current_style;

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
                            if !current_line.is_empty() {
                                self.lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
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
                                self.lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
                            self.lines.push(Line::from(""));
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
                        TagEnd::Strikethrough => {
                            current_style = current_style.remove_modifier(Modifier::CROSSED_OUT);
                        }
                        TagEnd::Emphasis => {
                            current_style = current_style.remove_modifier(Modifier::ITALIC);
                        }
                        TagEnd::CodeBlock => {
                            in_code_block = false;
                            if !current_line.is_empty() {
                                self.lines.push(Line::from(
                                    current_line
                                        .into_iter()
                                        .map(|s| Span::styled(s.content, Style::default().fg(colors::code_fg())))
                                        .collect::<Vec<_>>(),
                                ));
                                current_line = Vec::new();
                            }
                            self.lines.push(Line::from(Span::styled(
                                format!("{}{}", chars::CODE_BOTTOM_LEFT, chars::CODE_HORIZONTAL.repeat(40)),
                                Style::default().fg(colors::code_border()),
                            )));
                            code_language = None;
                        }
                        TagEnd::Item => {
                            // End of list item, push current line and add spacing
                            if !current_line.is_empty() {
                                self.lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
                        }
                        TagEnd::List(_) => {
                            list_stack.pop();
                            // Add empty line after list
                            if !self.lines.is_empty() {
                                self.lines.push(Line::from(""));
                            }
                        }
                        TagEnd::Heading(_) => {
                            if !current_line.is_empty() {
                                self.lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
                            self.lines.push(Line::from(""));
                            current_style = Style::default().fg(colors::text_primary());
                        }
                        TagEnd::Paragraph => {
                            if !current_line.is_empty() {
                                self.lines.push(Line::from(current_line));
                                current_line = Vec::new();
                            }
                            self.lines.push(Line::from(""));
                        }
                        _ => {}
                    }
                }
                MdEvent::Text(text) => {
                    if in_code_block {
                        for line in text.lines() {
                            if current_line.is_empty() && code_language.is_some() {
                                let lang = code_language.take().unwrap();
                                self.lines.push(Line::from(vec![
                                    Span::styled(
                                        format!("{}{} ", chars::CODE_TOP_LEFT, chars::CODE_HORIZONTAL.repeat(2)),
                                        Style::default().fg(colors::code_border()),
                                    ),
                                    Span::styled(lang, Styles::code_lang()),
                                ]));
                            }
                            if !current_line.is_empty() {
                                self.lines.push(Line::from(
                                    current_line
                                        .into_iter()
                                        .map(|s| Span::styled(s.content, Style::default().fg(colors::code_fg())))
                                        .collect::<Vec<_>>(),
                                ));
                                current_line = Vec::new();
                            }
                            self.lines.push(Line::from(vec![
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
                    current_line.push(Span::styled(
                        format!(" `{code}` "),
                        Styles::inline_code(),
                    ));
                }
                MdEvent::TaskListMarker(checked) => {
                    let checkbox = if checked { "[x]" } else { "[ ]" };
                    current_line.push(Span::styled(
                        format!("{checkbox} "),
                        Style::default().fg(if checked { colors::accent_user() } else { colors::text_secondary() }),
                    ));
                }
                MdEvent::SoftBreak | MdEvent::HardBreak => {
                    if in_code_block {
                        if !current_line.is_empty() {
                            self.lines.push(Line::from(
                                current_line
                                    .into_iter()
                                    .map(|s| Span::styled(s.content, Style::default().fg(colors::code_fg())))
                                    .collect::<Vec<_>>(),
                            ));
                            current_line = Vec::new();
                        }
                    } else if !current_line.is_empty() {
                        self.lines.push(Line::from(current_line));
                        current_line = Vec::new();
                    }
                }
                MdEvent::Rule => {
                    self.lines.push(Line::from(Span::styled(
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
                self.lines.push(Line::from(
                    current_line
                        .into_iter()
                        .map(|s| Span::styled(s.content, Style::default().fg(colors::code_fg())))
                        .collect::<Vec<_>>(),
                ));
            } else {
                self.lines.push(Line::from(current_line));
            }
        }

        // Remove trailing empty lines
        while self.lines.last().is_some_and(|l| l.to_string().trim().is_empty()) {
            self.lines.pop();
        }

        // Update state
        self.state = ParseState {
            in_code_block,
            code_language,
            list_stack,
            current_style,
        };
        self.dirty = false;

        &self.lines
    }
}
