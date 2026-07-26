//! Streaming markdown renderer with delta updates
//!
//! This renderer is optimized for streaming content, tracking state
//! and only re-rendering when necessary.

use pulldown_cmark::{CodeBlockKind, Event as MdEvent, Options, Parser, Tag, TagEnd};
use tuirealm::ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::table::StreamingTableRenderer;
use crate::theme::{chars, colors, Styles};
use crate::utils::text::preprocess;

/// Information about a rendered code block
#[derive(Debug, Clone)]
pub struct CodeBlockInfo {
    /// Starting line index (relative to current render output)
    pub start_line: usize,
    /// Ending line index (exclusive)
    pub end_line: usize,
    /// Programming language if specified
    pub language: Option<String>,
    /// Code content
    pub content: String,
}

impl CodeBlockInfo {
    /// Get the actual code content (excluding `` ``` `` markers)
    pub fn code_content(&self) -> &str {
        &self.content
    }
}

/// Tracks the state of markdown parsing for incremental rendering
#[derive(Debug, Clone, Copy)]
enum ListState {
    /// (`start_num`, `current_num`) for ordered lists
    Ordered(u64, u64),
    Unordered,
}

#[derive(Debug)]
struct ParseState {
    in_code_block: bool,
    code_language: Option<String>,
    list_stack: Vec<ListState>,
    current_style: Style,
    in_table: bool,
    in_table_head: bool,
}

impl Default for ParseState {
    fn default() -> Self {
        Self {
            in_code_block: false,
            code_language: None,
            list_stack: Vec::<ListState>::new(),
            current_style: Style::default().fg(colors::text_primary()),
            in_table: false,
            in_table_head: false,
        }
    }
}

/// Streaming markdown renderer that supports incremental updates
#[derive(Debug)]
pub struct StreamingMarkdownRenderer {
    content: String,
    lines: Vec<Line<'static>>,
    state: ParseState,
    table_renderer: Option<StreamingTableRenderer>,
    dirty: bool,
    /// Track code blocks for copy functionality
    code_blocks: Vec<CodeBlockInfo>,
    /// Throttle full re-parsing during streaming to avoid O(n²) behaviour.
    last_render_at: std::time::Instant,
    /// Viewport width for pre-wrapped block elements (tables). Lines are
    /// wrapped later by `WrapParagraph`, but tables must be laid out at a
    /// fixed width during rendering.
    width: usize,
}

impl Default for StreamingMarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingMarkdownRenderer {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            lines: Vec::new(),
            state: ParseState::default(),
            table_renderer: None,
            dirty: false,
            code_blocks: Vec::new(),
            last_render_at: std::time::Instant::now(),
            width: 120,
        }
    }

    /// Set the viewport width for pre-wrapped elements (tables).
    /// Marks the renderer dirty so the next `lines()` re-renders.
    pub fn set_width(&mut self, width: usize) {
        if width > 0 && width != self.width {
            self.width = width;
            self.dirty = true;
        }
    }

    /// Get the list of code blocks detected during rendering
    pub fn code_blocks(&self) -> &[CodeBlockInfo] {
        &self.code_blocks
    }

    /// Append new text and re-render (throttled to avoid O(n²) re-parsing).
    pub fn append(&mut self, text: &str) -> &[Line<'static>] {
        if text.is_empty() {
            return &self.lines;
        }

        self.content.push_str(text);
        self.dirty = true;
        // Only re-parse if >50ms since last render. The caller polls
        // `lines()` regularly, so any trailing content is flushed there.
        if self.last_render_at.elapsed() > std::time::Duration::from_millis(50) {
            self.render();
        }
        &self.lines
    }

    /// Set content and re-render
    pub fn set_content(&mut self, content: String) -> &[Line<'static>] {
        self.content = content;
        self.lines.clear();
        self.state = ParseState::default();
        self.table_renderer = None;
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
        self.code_blocks.clear();

        let options =
            Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;

        let parser = Parser::new_ext(&self.content, options);
        let parser_iter = parser.peekable();

        let mut current_line: Vec<Span> = Vec::new();
        let mut in_code_block = self.state.in_code_block;
        let mut code_language = self.state.code_language.clone();
        let mut code_block_start: Option<usize> = None;
        let mut code_block_content: String = String::new();
        let mut list_stack: Vec<ListState> = self.state.list_stack.clone();
        let mut current_style = self.state.current_style;
        let mut in_table = self.state.in_table;
        let mut in_table_head = self.state.in_table_head;

        // Reset table renderer if needed
        if self.table_renderer.is_none() && in_table {
            self.table_renderer = Some(StreamingTableRenderer::new());
        }

        for event in parser_iter {
            match event {
                MdEvent::Start(tag) => match tag {
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
                        code_block_start = Some(self.lines.len());
                        code_block_content.clear();
                        if !current_line.is_empty() {
                            self.lines.push(Line::from(current_line));
                            current_line = Vec::new();
                        }
                        if let CodeBlockKind::Fenced(lang) = kind {
                            code_language = Some(lang.to_string());
                        }
                    }
                    Tag::List(start_num) => {
                        list_stack.push(
                            start_num.map_or(ListState::Unordered, |n| ListState::Ordered(n, n)),
                        );
                    }
                    Tag::Item => {
                        let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                        let prefix = match list_stack.last_mut() {
                            Some(ListState::Ordered(_start, current)) => {
                                let num = *current;
                                *current += 1;
                                format!("{indent}{num}. ")
                            }
                            Some(ListState::Unordered) => {
                                format!("{indent}{} ", chars::BULLET)
                            }
                            None => format!("{} ", chars::BULLET),
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
                        // Add heading prefix (###)
                        let prefix = match level {
                            pulldown_cmark::HeadingLevel::H1 => "# ",
                            pulldown_cmark::HeadingLevel::H2 => "## ",
                            pulldown_cmark::HeadingLevel::H3 => "### ",
                            pulldown_cmark::HeadingLevel::H4 => "#### ",
                            pulldown_cmark::HeadingLevel::H5 => "##### ",
                            pulldown_cmark::HeadingLevel::H6 => "###### ",
                        };
                        current_line.push(Span::styled(
                            prefix,
                            Style::default()
                                .fg(colors::text_primary())
                                .add_modifier(Modifier::BOLD),
                        ));
                        current_style = Style::default()
                            .fg(colors::text_primary())
                            .add_modifier(Modifier::BOLD);
                    }
                    Tag::BlockQuote(_) => {
                        current_line.push(Span::styled(
                            chars::MSG_INDENT_GUIDE,
                            Style::default().fg(colors::border()),
                        ));
                    }
                    Tag::Table(_) => {
                        in_table = true;
                        // Flush any pending content before table
                        if !current_line.is_empty() {
                            self.lines.push(Line::from(current_line));
                            current_line = Vec::new();
                        }
                        self.table_renderer = Some(StreamingTableRenderer::new());
                        if let Some(ref mut tr) = self.table_renderer {
                            tr.start_table();
                        }
                    }
                    Tag::TableHead => {
                        in_table_head = true;
                        if let Some(ref mut tr) = self.table_renderer {
                            tr.start_head();
                        }
                    }
                    Tag::TableRow => {
                        if let Some(ref mut tr) = self.table_renderer {
                            tr.start_row();
                        }
                    }
                    Tag::TableCell => {
                        if let Some(ref mut tr) = self.table_renderer {
                            tr.start_cell();
                        }
                    }
                    _ => {}
                },
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
                                        .map(|s| {
                                            Span::styled(
                                                s.content,
                                                Style::default().fg(colors::code_fg()),
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                ));
                                current_line = Vec::new();
                            }
                            // Record code block info
                            if let Some(start) = code_block_start {
                                self.code_blocks.push(CodeBlockInfo {
                                    start_line: start,
                                    end_line: self.lines.len(),
                                    language: code_language.clone(),
                                    content: code_block_content.clone(),
                                });
                            }
                            // Show closing ```
                            self.lines
                                .push(Line::from(Span::styled("```", Styles::code_lang())));
                            code_language = None;
                            code_block_start = None;
                            code_block_content.clear();
                        }
                        TagEnd::Item
                            // End of list item, push current line and add spacing
                            if !current_line.is_empty() => {
                                self.lines.push(Line::from(current_line));
                                current_line = Vec::new();
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
                        TagEnd::Table => {
                            in_table = false;
                            in_table_head = false;
                            // Render the complete table
                            if let Some(ref tr) = self.table_renderer {
                                let table_lines = tr.render(self.width);
                                for line in table_lines {
                                    self.lines.push(line);
                                }
                            }
                            self.table_renderer = None;
                        }
                        TagEnd::TableHead => {
                            in_table_head = false;
                            if let Some(ref mut tr) = self.table_renderer {
                                tr.end_head();
                            }
                        }
                        TagEnd::TableRow => {
                            if let Some(ref mut tr) = self.table_renderer {
                                tr.end_row();
                            }
                        }
                        TagEnd::TableCell => {
                            if let Some(ref mut tr) = self.table_renderer {
                                tr.end_cell();
                            }
                        }
                        _ => {}
                    }
                }
                MdEvent::Text(text) => {
                    if in_code_block {
                        for line in text.lines() {
                            // Accumulate code content for copy functionality
                            if !code_block_content.is_empty() {
                                code_block_content.push('\n');
                            }
                            code_block_content.push_str(line);

                            // Show ```{lang} at start of code block
                            if current_line.is_empty() && code_language.is_some() {
                                let lang = code_language.take().unwrap();
                                self.lines.push(Line::from(Span::styled(
                                    format!("```{lang}"),
                                    Styles::code_lang(),
                                )));
                            }
                            if !current_line.is_empty() {
                                self.lines.push(Line::from(
                                    current_line
                                        .into_iter()
                                        .map(|s| {
                                            Span::styled(
                                                s.content,
                                                Style::default().fg(colors::code_fg()),
                                            )
                                        })
                                        .collect::<Vec<_>>(),
                                ));
                                current_line = Vec::new();
                            }
                            // Simple code line without border
                            let expanded_line = line.replace('\t', "  ");
                            self.lines.push(Line::from(Span::styled(
                                expanded_line,
                                Style::default().fg(colors::code_fg()).bg(colors::code_bg()),
                            )));
                        }
                    } else if in_table {
                        if let Some(ref mut tr) = self.table_renderer {
                            tr.append_text(&text);
                        }
                    } else {
                        current_line.push(Span::styled(preprocess(&text), current_style));
                    }
                }
                MdEvent::Code(code) => {
                    if in_table {
                        if let Some(ref mut tr) = self.table_renderer {
                            tr.append_text(&code);
                        }
                    } else {
                        // Inline code should inherit modifiers (italic, underline, etc.) from context
                        let style = Styles::inline_code().patch(current_style);
                        current_line.push(Span::styled(format!("{code}"), style));
                    }
                }
                MdEvent::InlineHtml(html) => {
                    let html_str = html.to_string();
                    match html_str.as_str() {
                        "<u>" | "<ins>" | "<underline>" => {
                            current_style = current_style.add_modifier(Modifier::UNDERLINED);
                        }
                        "</u>" | "</ins>" | "</underline>" => {
                            current_style = current_style.remove_modifier(Modifier::UNDERLINED);
                        }
                        _ => {
                            // Pass through other HTML tags as text
                            current_line.push(Span::styled(html_str, current_style));
                        }
                    }
                }
                MdEvent::TaskListMarker(checked) => {
                    let checkbox = if checked { "[x]" } else { "[ ]" };
                    current_line.push(Span::styled(
                        format!("{checkbox} "),
                        Style::default().fg(if checked {
                            colors::accent_user()
                        } else {
                            colors::text_secondary()
                        }),
                    ));
                }
                MdEvent::SoftBreak | MdEvent::HardBreak => {
                    if in_code_block {
                        if !current_line.is_empty() {
                            self.lines.push(Line::from(
                                current_line
                                    .into_iter()
                                    .map(|s| {
                                        Span::styled(
                                            s.content,
                                            Style::default().fg(colors::code_fg()),
                                        )
                                    })
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
                    // Horizontal divider line using box-drawing character
                    self.lines.push(Line::from(Span::styled(
                        "─".repeat(40),
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

        // If we're still in a table (incomplete), render current state
        if in_table {
            if let Some(ref tr) = self.table_renderer {
                // Clear previous partial table lines and re-render
                // Find where table starts (look for last empty line or beginning)
                let table_start = self
                    .lines
                    .iter()
                    .rposition(|l| l.to_string().trim().is_empty())
                    .map_or(0, |i| i + 1);

                let table_lines = tr.render(self.width);
                self.lines.truncate(table_start);
                for line in table_lines {
                    self.lines.push(line);
                }
            }
        }

        // Remove trailing empty lines
        while self
            .lines
            .last()
            .is_some_and(|l| l.to_string().trim().is_empty())
        {
            self.lines.pop();
        }

        // Update state
        self.state = ParseState {
            in_code_block,
            code_language,
            list_stack,
            current_style,
            in_table,
            in_table_head,
        };
        self.dirty = false;
        self.last_render_at = std::time::Instant::now();

        &self.lines
    }
}

#[cfg(test)]
#[path = "markdown_stream_test.rs"]
mod tests;
