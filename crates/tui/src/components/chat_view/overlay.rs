//! Code block copy button overlay
//!
//! Displays a small copy button next to code block headers.
//! Manages positioning, rendering, and click detection.

use std::sync::Arc;

use tuirealm::ratatui::{
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Clear, Paragraph},
    Frame,
};

use crate::theme::colors;

/// Convert a `Line` to its text content efficiently.
/// Pre-calculates total length to allocate exactly once.
pub fn line_to_text(line: &Arc<Line<'static>>) -> String {
    let total_len: usize = line.spans.iter().map(|s| s.content.len()).sum();
    let mut result = String::with_capacity(total_len);
    for span in &line.spans {
        result.push_str(&span.content);
    }
    result
}

/// A copy button overlay for a single code block
#[derive(Debug, Clone)]
pub struct CodeBlockOverlay {
    /// Screen position of the button
    rect: Rect,
    /// Code content to copy
    content: String,
}

impl CodeBlockOverlay {
    const BUTTON_WIDTH: u16 = 2; // "⧉ "
    const BUTTON_HEIGHT: u16 = 1;
    const SYMBOL: &'static str = "";

    /// Create a new overlay at the given visual line position
    ///
    /// # Arguments
    /// * `visual_line` - Line index relative to visible area (0 = first visible row)
    /// * `content` - Code content to copy
    /// * `area` - The display area
    /// * `header_width` - Width of the `` `lang `` header text
    pub fn new(visual_line: usize, content: String, area: Rect, header_width: u16) -> Option<Self> {
        if visual_line >= area.height as usize {
            return None;
        }

        let y = area.y + visual_line as u16;
        // Position right after the header text (```lang) with 1 char margin
        let x = area.x + header_width + 1;

        // Ensure button doesn't overflow the area
        if x + Self::BUTTON_WIDTH > area.x + area.width {
            return None;
        }

        Some(Self {
            rect: Rect::new(x, y, Self::BUTTON_WIDTH, Self::BUTTON_HEIGHT),
            content,
        })
    }

    /// Check if point is inside the button
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.rect.contains(Position::new(x, y))
    }

    /// Get the code content
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Render the copy button
    pub fn render(&self, frame: &mut Frame) {
        // Clear the area
        frame.render_widget(Clear, self.rect);

        // Render button with accent_system color for consistency
        let button = Paragraph::new(Self::SYMBOL).style(
            Style::default()
                .fg(colors::accent_system())
                .add_modifier(Modifier::BOLD),
        );

        frame.render_widget(button, self.rect);
    }
}

/// Manager for all visible code block overlays
#[derive(Debug, Default)]
pub struct CodeBlockOverlayManager {
    overlays: Vec<CodeBlockOverlay>,
}

impl CodeBlockOverlayManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all overlays
    pub fn clear(&mut self) {
        self.overlays.clear();
    }

    /// Add an overlay
    pub fn push(&mut self, overlay: CodeBlockOverlay) {
        self.overlays.push(overlay);
    }

    /// Find overlay at the given coordinates and return its content if found
    pub fn find_and_copy(&self, x: u16, y: u16) -> Option<&str> {
        for overlay in &self.overlays {
            if overlay.contains(x, y) {
                return Some(overlay.content());
            }
        }
        None
    }

    /// Render all overlays
    pub fn render_all(&self, frame: &mut Frame) {
        for overlay in &self.overlays {
            overlay.render(frame);
        }
    }

    /// Returns true if there are no overlays
    pub fn is_empty(&self) -> bool {
        self.overlays.is_empty()
    }

    /// Get the number of overlays
    pub fn len(&self) -> usize {
        self.overlays.len()
    }
}

/// Scan lines to find code block headers and their content
///
/// Returns a vector of (`line_index`, `code_content`) tuples.
/// Note: Unclosed code blocks at the end are not included (expected in streaming).
pub fn scan_code_blocks(lines: &[Arc<Line<'static>>]) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut in_code_block = false;
    let mut block_start = 0;
    let mut block_content = String::new();

    for (i, line) in lines.iter().enumerate() {
        let line_text = line_to_text(line);

        if line_text.starts_with("```") && !line_text.get(3..).is_some_and(|s| s.starts_with('`')) {
            if in_code_block {
                // End of code block - move content to avoid clone
                in_code_block = false;
                blocks.push((block_start, std::mem::take(&mut block_content)));
            } else {
                // Start of code block
                in_code_block = true;
                block_start = i;
                block_content.clear();
            }
        } else if in_code_block {
            // Accumulate code content
            if !block_content.is_empty() {
                block_content.push('\n');
            }
            block_content.push_str(&line_text);
        }
    }

    blocks
}

/// Convert logical line index to visual line index (accounting for wrapping)
pub fn logical_to_visual_line(
    lines: &[Arc<Line<'static>>],
    target_logical: usize,
    width: usize,
    calculate_wrap_boundaries: impl Fn(&str, usize) -> Vec<usize>,
) -> usize {
    let mut visual_line = 0usize;

    for (i, line) in lines.iter().enumerate() {
        if i >= target_logical {
            break;
        }
        let line_text = line_to_text(line);
        let wrapped_height = calculate_wrap_boundaries(&line_text, width).len();
        visual_line += wrapped_height.max(1);
    }

    visual_line
}

/// Get the display width of a line
pub fn line_display_width(line: &Arc<Line<'static>>) -> u16 {
    let line_text = line_to_text(line);
    unicode_width::UnicodeWidthStr::width(line_text.as_str()) as u16
}

/// Context menu actions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    /// Copy the raw message content
    CopyContent,
    /// Copy as pretty JSON
    CopyPrettyJson,
}

/// Context menu item
#[derive(Debug, Clone)]
pub struct ContextMenuItem {
    /// Display label
    pub label: &'static str,
    /// Action to perform
    pub action: ContextMenuAction,
}

impl ContextMenuItem {
    const ITEMS: [ContextMenuItem; 2] = [
        ContextMenuItem {
            label: "Copy content",
            action: ContextMenuAction::CopyContent,
        },
        ContextMenuItem {
            label: "Copy pretty JSON",
            action: ContextMenuAction::CopyPrettyJson,
        },
    ];

    /// Get all menu items
    pub fn all() -> &'static [ContextMenuItem] {
        &Self::ITEMS
    }
}

/// Context menu overlay for message actions
#[derive(Debug, Clone)]
pub struct ContextMenu {
    /// Screen position of the menu
    rect: Rect,
    /// Message index this menu is for
    pub message_idx: usize,
    /// Currently hovered item index (None if none)
    pub hovered_idx: Option<usize>,
}

impl ContextMenu {
    /// Create a new context menu at the given screen position
    pub fn new(x: u16, y: u16, message_idx: usize, area: Rect) -> Option<Self> {
        let items = ContextMenuItem::all();
        // Height: items + top/bottom border (no padding)
        let height = items.len() as u16 + 2;

        // Calculate max label width
        let max_label_width = items
            .iter()
            .map(|item| unicode_width::UnicodeWidthStr::width(item.label))
            .max()
            .unwrap_or(0) as u16;
        // Width: content + left/right border + small padding
        let width = max_label_width + 4;

        // Adjust position to stay within area
        let mut menu_x = x;
        let mut menu_y = y;

        if menu_x + width > area.x + area.width {
            menu_x = area.x + area.width - width;
        }
        if menu_y + height > area.y + area.height {
            menu_y = y.saturating_sub(height);
        }

        // Ensure we don't go above the area
        if menu_y < area.y {
            menu_y = area.y;
        }

        Some(Self {
            rect: Rect::new(menu_x, menu_y, width, height),
            message_idx,
            hovered_idx: None,
        })
    }

    /// Get the rectangle for a specific item (directly inside border, no extra padding)
    fn item_rect(&self, idx: usize) -> Rect {
        Rect::new(
            self.rect.x + 2,                   // Left border + 1 space padding
            self.rect.y + 1 + idx as u16,      // Top border + item index
            self.rect.width.saturating_sub(4), // Remove left/right borders and padding
            1,
        )
    }

    /// Check if point is inside the menu
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.rect.contains(Position::new(x, y))
    }

    /// Update hovered item based on mouse position
    pub fn update_hover(&mut self, x: u16, y: u16) {
        if !self.contains(x, y) {
            self.hovered_idx = None;
            return;
        }

        let items = ContextMenuItem::all();
        for (idx, _) in items.iter().enumerate() {
            let rect = self.item_rect(idx);
            if rect.contains(Position::new(x, y)) {
                self.hovered_idx = Some(idx);
                return;
            }
        }
        self.hovered_idx = None;
    }

    /// Get the action at the given position, if any
    pub fn get_action_at(&self, x: u16, y: u16) -> Option<ContextMenuAction> {
        if !self.contains(x, y) {
            return None;
        }

        let items = ContextMenuItem::all();
        for (idx, item) in items.iter().enumerate() {
            let rect = self.item_rect(idx);
            if rect.contains(Position::new(x, y)) {
                return Some(item.action);
            }
        }
        None
    }

    /// Render the context menu
    pub fn render(&self, frame: &mut Frame) {
        use tuirealm::ratatui::{
            symbols,
            widgets::{Block, Borders},
        };

        let items = ContextMenuItem::all();

        // Fill the entire menu area with spaces to cover content behind
        let spaces = " ".repeat(self.rect.width as usize);
        for y in self.rect.y..self.rect.y + self.rect.height {
            let line_rect = Rect::new(self.rect.x, y, self.rect.width, 1);
            frame.render_widget(
                Paragraph::new(spaces.clone()).style(Style::default().fg(colors::text_primary())),
                line_rect,
            );
        }

        // Render rounded border with emoji title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(symbols::border::ROUNDED)
            .border_style(Style::default().fg(colors::accent_system()))
            .title(" ");
        frame.render_widget(block, self.rect);

        // Render each item
        for (idx, item) in items.iter().enumerate() {
            let rect = self.item_rect(idx);
            let is_hovered = self.hovered_idx == Some(idx);

            let style = if is_hovered {
                Style::default()
                    .fg(colors::accent_system())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(colors::text_primary())
            };

            let paragraph = Paragraph::new(item.label).style(style);
            frame.render_widget(paragraph, rect);
        }
    }
}
