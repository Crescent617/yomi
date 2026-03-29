# TUI Redesign Implementation Plan

**Goal:** Redesign TUI to be lightweight and beautiful like Claude Code

**Architecture:**
- New input system with multi-line support and Emacs shortcuts
- Folding state management for Tool/Thinking sections
- Streamlined rendering without assistant prefix
- Double Ctrl+C exit mechanism

---

## File Structure

| File | Changes |
|------|---------|
| `crates/tui/src/app.rs` | Complete rewrite of UI logic |
| `crates/tui/src/input.rs` | New multi-line input handler |
| `crates/tui/src/fold.rs` | Folding state management |
| `crates/tui/src/render.rs` | Rendering logic |
| `crates/tui/src/lib.rs` | Export new modules |

---

## Task 1: Create Input Module

**Files:**
- Create: `crates/tui/src/input.rs`

- [ ] **Step 1: Create InputBuffer struct**

```rust
use std::collections::VecDeque;
use unicode_width::UnicodeWidthStr;

pub struct InputBuffer {
    lines: Vec<String>,
    cursor_line: usize,
    cursor_col: usize,  // byte position
    history: VecDeque<String>,
    history_index: Option<usize>,
    max_history: usize,
}

impl InputBuffer {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            history: VecDeque::with_capacity(100),
            history_index: None,
            max_history: 100,
        }
    }

    /// Insert character at cursor
    pub fn insert(&mut self, c: char) {
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = line.char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        line.insert(byte_idx, c);
        self.cursor_col += 1;
    }

    /// Insert newline (Ctrl+J)
    pub fn insert_newline(&mut self) {
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = line.char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        let remainder: String = line.split_off(byte_idx);
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.lines.insert(self.cursor_line, remainder);
    }

    /// Delete backward word (Ctrl+W)
    pub fn delete_word(&mut self) {
        let line = &self.lines[self.cursor_line];
        if self.cursor_col == 0 {
            if self.cursor_line > 0 {
                // Join with previous line
                let current = self.lines.remove(self.cursor_line);
                self.cursor_line -= 1;
                let prev = &mut self.lines[self.cursor_line];
                self.cursor_col = prev.chars().count();
                prev.push_str(&current);
            }
            return;
        }

        let byte_idx = line.char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());

        // Find word boundary
        let prev_text = &line[..byte_idx];
        let new_col = prev_text
            .chars()
            .rev()
            .skip_while(|c| c.is_whitespace())
            .skip_while(|c| !c.is_whitespace())
            .count();

        let new_byte_idx = line.char_indices()
            .nth(new_col)
            .map(|(i, _)| i)
            .unwrap_or(0);

        self.lines[self.cursor_line].drain(new_byte_idx..byte_idx);
        self.cursor_col = new_col;
    }

    /// Delete to start of line (Ctrl+U)
    pub fn delete_to_start(&mut self) {
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = line.char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        line.drain(..byte_idx);
        self.cursor_col = 0;
    }

    /// Delete to end of line (Ctrl+K)
    pub fn delete_to_end(&mut self) {
        let line = &mut self.lines[self.cursor_line];
        let byte_idx = line.char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        line.truncate(byte_idx);
    }

    /// Move cursor to line start (Ctrl+A)
    pub fn move_to_start(&mut self) {
        self.cursor_col = 0;
    }

    /// Move cursor to line end (Ctrl+E)
    pub fn move_to_end(&mut self) {
        self.cursor_col = self.lines[self.cursor_line].chars().count();
    }

    /// Get content as single string with newlines
    pub fn content(&self) -> String {
        self.lines.join("\n")
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Clear and save to history
    pub fn commit(&mut self) {
        let content = self.content();
        if !content.trim().is_empty() {
            if self.history.len() >= self.max_history {
                self.history.pop_back();
            }
            self.history.push_front(content);
        }
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
        self.history_index = None;
    }

    /// Navigate history up (Ctrl+P)
    pub fn history_prev(&mut self) {
        if self.history.is_empty() { return; }

        let idx = self.history_index.map(|i| (i + 1).min(self.history.len() - 1))
            .unwrap_or(0);

        if idx < self.history.len() {
            self.history_index = Some(idx);
            let content = self.history[idx].clone();
            self.lines = content.lines().map(|s| s.to_string()).collect();
            if self.lines.is_empty() {
                self.lines.push(String::new());
            }
            self.cursor_line = self.lines.len() - 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
        }
    }

    /// Navigate history down (Ctrl+N)
    pub fn history_next(&mut self) {
        let idx = match self.history_index {
            None => return,
            Some(0) => {
                self.history_index = None;
                self.lines = vec![String::new()];
                self.cursor_line = 0;
                self.cursor_col = 0;
                return;
            }
            Some(i) => i - 1,
        };

        self.history_index = Some(idx);
        let content = self.history[idx].clone();
        self.lines = content.lines().map(|s| s.to_string()).collect();
        self.cursor_line = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_line].chars().count();
    }

    /// Get cursor display column (for CJK)
    pub fn cursor_display_col(&self) -> usize {
        self.lines[self.cursor_line][..self.byte_col()].width()
    }

    fn byte_col(&self) -> usize {
        self.lines[self.cursor_line]
            .char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(self.lines[self.cursor_line].len())
    }

    pub fn lines(&self) -> &[String] { &self.lines }
    pub fn cursor_line(&self) -> usize { self.cursor_line }
    pub fn cursor_col(&self) -> usize { self.cursor_col }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/tui/src/input.rs
git commit -m "feat: add multi-line input buffer with Emacs shortcuts

- InputBuffer for multi-line text editing
- Ctrl+W: delete word, Ctrl+U: delete to start
- Ctrl+K: delete to end, Ctrl+A/E: line start/end
- History navigation with Ctrl+P/N"
```

---

## Task 2: Create Folding Module

**Files:**
- Create: `crates/tui/src/fold.rs`

- [ ] **Step 1: Create FoldState for collapsible sections**

```rust
use crate::app::MessageId;

/// Types of collapsible content
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldableType {
    Tools,
    Thinking,
}

/// State for a foldable section
#[derive(Debug, Clone)]
pub struct FoldState {
    pub id: MessageId,
    pub fold_type: FoldableType,
    pub is_expanded: bool,
    pub summary: String,
    pub token_count: Option<usize>,
}

/// Manages folding state for all collapsible sections
#[derive(Debug, Default)]
pub struct FoldManager {
    folds: Vec<FoldState>,
    focused_index: Option<usize>,
}

impl FoldManager {
    pub fn new() -> Self {
        Self {
            folds: Vec::new(),
            focused_index: None,
        }
    }

    /// Register a new foldable section
    pub fn register(
        &mut self,
        id: MessageId,
        fold_type: FoldableType,
        summary: impl Into<String>,
        token_count: Option<usize>,
    ) {
        self.folds.push(FoldState {
            id,
            fold_type,
            is_expanded: false, // Default collapsed
            summary: summary.into(),
            token_count,
        });
    }

    /// Toggle fold at index
    pub fn toggle(&mut self, index: usize) {
        if let Some(fold) = self.folds.get_mut(index) {
            fold.is_expanded = !fold.is_expanded;
        }
    }

    /// Toggle fold by message ID
    pub fn toggle_by_id(&mut self, id: MessageId) -> bool {
        if let Some(fold) = self.folds.iter_mut().find(|f| f.id == id) {
            fold.is_expanded = !fold.is_expanded;
            true
        } else {
            false
        }
    }

    /// Get fold state
    pub fn get(&self, id: MessageId) -> Option<&FoldState> {
        self.folds.iter().find(|f| f.id == id)
    }

    /// Check if expanded
    pub fn is_expanded(&self, id: MessageId) -> bool {
        self.get(id).map(|f| f.is_expanded).unwrap_or(false)
    }

    /// Navigate to next fold (for Tab navigation)
    pub fn next_fold(&mut self) {
        self.focused_index = match self.focused_index {
            None => Some(0),
            Some(i) => Some((i + 1) % self.folds.len()),
        };
    }

    /// Get currently focused fold ID
    pub fn focused_id(&self) -> Option<MessageId> {
        self.focused_index
            .and_then(|i| self.folds.get(i))
            .map(|f| f.id)
    }

    /// Render fold indicator
    pub fn render_indicator(fold: &FoldState) -> String {
        let icon = if fold.is_expanded { "▼" } else { "▶" };
        match fold.fold_type {
            FoldableType::Tools => {
                format!("{} {}", icon, fold.summary)
            }
            FoldableType::Thinking => {
                let tokens = fold.token_count
                    .map(|n| format!("({} tokens)", n))
                    .unwrap_or_default();
                format!("{} Thinking {}", icon, tokens)
            }
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/tui/src/fold.rs
git commit -m "feat: add foldable section manager

- FoldManager for Tool/Thinking collapsible sections
- Default collapsed state
- Tab navigation between folds"
```

---

## Task 3: Create Render Module

**Files:**
- Create: `crates/tui/src/render.rs`

- [ ] **Step 1: Create rendering functions**

```rust
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::app::{ChatMessage, Role};
use crate::fold::{FoldManager, FoldState};
use crate::markdown::MarkdownRenderer;

/// Color constants
pub const COLOR_USER: Color = Color::Green;
pub const COLOR_ASSISTANT: Color = Color::White;
pub const COLOR_TOOL_NAME: Color = Color::Blue;
pub const COLOR_TOOL_BORDER: Color = Color::DarkGray;
pub const COLOR_THINKING: Color = Color::Gray;
pub const COLOR_SYSTEM: Color = Color::DarkGray;
pub const COLOR_CODE_BG: Color = Color::Rgb(40, 40, 40);

/// Render user message
pub fn render_user(lines: &mut Vec<Line>, content: &str) {
    for (i, line) in content.lines().enumerate() {
        let prefix = if i == 0 { "> " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(COLOR_USER).add_modifier(Modifier::BOLD)),
            Span::styled(line.to_string(), Style::default().fg(COLOR_USER)),
        ]));
    }
}

/// Render assistant message (no prefix)
pub fn render_assistant(
    lines: &mut Vec<Line>,
    content: &str,
    thinking: Option<&str>,
    fold_manager: Option<&FoldManager>,
    msg_id: crate::app::MessageId,
) {
    let markdown = MarkdownRenderer::new();

    // Render thinking if present
    if let Some(thinking) = thinking {
        let is_expanded = fold_manager
            .map(|fm| fm.is_expanded(msg_id))
            .unwrap_or(false);

        if is_expanded {
            lines.push(Line::from(vec![
                Span::styled("▼ Thinking", Style::default().fg(COLOR_THINKING)),
            ]));
            for line in thinking.lines() {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {}", line),
                        Style::default().fg(COLOR_THINKING).add_modifier(Modifier::ITALIC)
                    ),
                ]));
            }
        } else {
            let tokens = thinking.len() / 4; // Rough estimate
            lines.push(Line::from(vec![
                Span::styled(
                    format!("▶ Thinking ({} tokens)", tokens),
                    Style::default().fg(COLOR_THINKING)
                ),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Render main content
    let md_lines = markdown.render(content);
    lines.extend(md_lines);
}

/// Render tool output with box drawing
pub fn render_tool(
    lines: &mut Vec<Line>,
    tool_name: &str,
    tool_input: &str,
    tool_output: &str,
    is_expanded: bool,
) {
    if !is_expanded {
        lines.push(Line::from(vec![
            Span::styled(
                format!("▶ Tool: {} {}", tool_name, tool_input),
                Style::default().fg(COLOR_TOOL_NAME)
            ),
        ]));
        return;
    }

    // Expanded view with box
    let header = format!("┌─ {}: {} ", tool_name, tool_input);
    let width = 60usize; // Configurable
    let padding = width.saturating_sub(header.len());
    let border = format!("{}{}┐", header, "─".repeat(padding));

    lines.push(Line::from(vec![
        Span::styled(border, Style::default().fg(COLOR_TOOL_BORDER)),
    ]));

    for line in tool_output.lines() {
        let truncated = if line.len() > width - 2 {
            format!("{}..", &line[..width-4])
        } else {
            line.to_string()
        };
        let padding = width.saturating_sub(truncated.len() + 2);
        let formatted = format!("│ {}{}│", truncated, " ".repeat(padding));
        lines.push(Line::from(vec![
            Span::styled(formatted, Style::default().fg(COLOR_TOOL_BORDER)),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled(format!("└{}┘", "─".repeat(width)), Style::default().fg(COLOR_TOOL_BORDER)),
    ]));
}

/// Render system message
pub fn render_system(lines: &mut Vec<Line>, content: &str) {
    lines.push(Line::from(vec![
        Span::styled(
            content.to_string(),
            Style::default().fg(COLOR_SYSTEM).add_modifier(Modifier::ITALIC)
        ),
    ]));
}

/// Render input line
pub fn render_input(input_lines: &[String], cursor_line: usize, cursor_col: usize) -> Vec<Line> {
    let mut lines = Vec::new();

    for (i, line) in input_lines.iter().enumerate() {
        let prefix = if i == 0 { "> " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(COLOR_USER).add_modifier(Modifier::BOLD)),
            Span::styled(line.clone(), Style::default().fg(COLOR_ASSISTANT)),
        ]));
    }

    // If empty, show just prompt
    if input_lines.is_empty() || (input_lines.len() == 1 && input_lines[0].is_empty()) {
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(COLOR_USER).add_modifier(Modifier::BOLD)),
            Span::styled("_", Style::default().fg(COLOR_USER)),
        ]));
    }

    lines
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/tui/src/render.rs
git commit -m "feat: add rendering functions for new TUI style

- render_user: green with > prefix
- render_assistant: no prefix, optional thinking
- render_tool: box-drawing borders
- render_system: gray italic"
```

---

## Task 4: Rewrite Main App Module

**Files:**
- Modify: `crates/tui/src/app.rs`

- [ ] **Step 1: Update imports and types**

```rust
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
    ExecutableCommand,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame, Terminal,
};
use std::io::{self, stdout};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use unicode_width::UnicodeWidthStr;

use nekoclaw_core::bus::EventBus;
use nekoclaw_core::event::Event as AppEvent;

use crate::fold::FoldManager;
use crate::input::InputBuffer;
use crate::render;

pub type MessageId = usize;

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: MessageId,
    pub role: Role,
    pub content: String,
    pub thinking: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub input: String,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role {
    User,
    Assistant,
    System,
}

pub struct App {
    event_rx: broadcast::Receiver<AppEvent>,
    input_tx: tokio::sync::mpsc::Sender<String>,
    should_quit: bool,
    input: InputBuffer,
    messages: Vec<ChatMessage>,
    next_msg_id: MessageId,
    scroll_offset: usize,
    streaming_content: String,
    streaming_thinking: String,
    is_streaming: bool,
    fold_manager: FoldManager,
    last_ctrl_c: Option<Instant>,
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}
```

- [ ] **Step 2: Implement double Ctrl+C detection**

```rust
const CTRL_C_THRESHOLD: Duration = Duration::from_millis(1000);

impl App {
    fn handle_ctrl_c(&mut self) -> bool {
        let now = Instant::now();

        if let Some(last) = self.last_ctrl_c {
            if now.duration_since(last) < CTRL_C_THRESHOLD {
                // Double press - quit
                self.should_quit = true;
                return true;
            }
        }

        // Single press - cancel current task
        self.last_ctrl_c = Some(now);

        if self.is_streaming {
            // Send cancel signal
            let _ = self.input_tx.try_send("__CANCEL__".to_string());
        }

        false
    }
}
```

- [ ] **Step 3: Implement key handling**

```rust
async fn handle_key(
    &mut self,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<bool> {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);

    match (code, ctrl) {
        // Ctrl+C - cancel/quit
        (KeyCode::Char('c'), true) => {
            if self.handle_ctrl_c() {
                return Ok(true);
            }
            // Show "Press Ctrl+C again to quit" message
        }

        // Ctrl+J - newline
        (KeyCode::Char('j'), true) => {
            self.input.insert_newline();
        }

        // Ctrl+W - delete word
        (KeyCode::Char('w'), true) => {
            self.input.delete_word();
        }

        // Ctrl+U - delete to start
        (KeyCode::Char('u'), true) => {
            self.input.delete_to_start();
        }

        // Ctrl+K - delete to end
        (KeyCode::Char('k'), true) => {
            self.input.delete_to_end();
        }

        // Ctrl+A - start of line
        (KeyCode::Char('a'), true) => {
            self.input.move_to_start();
        }

        // Ctrl+E - end of line
        (KeyCode::Char('e'), true) => {
            self.input.move_to_end();
        }

        // Ctrl+P - previous history
        (KeyCode::Char('p'), true) | (KeyCode::Up, false) => {
            self.input.history_prev();
        }

        // Ctrl+N - next history
        (KeyCode::Char('n'), true) | (KeyCode::Down, false) => {
            self.input.history_next();
        }

        // Ctrl+L - clear screen
        (KeyCode::Char('l'), true) => {
            stdout().execute(Clear(ClearType::All))?;
        }

        // Tab - toggle fold
        (KeyCode::Tab, false) => {
            if let Some(id) = self.fold_manager.focused_id() {
                self.fold_manager.toggle_by_id(id);
            }
        }

        // Enter - send if not empty
        (KeyCode::Enter, false) => {
            if !self.input.is_empty() {
                let content = self.input.content();
                self.input_tx.send(content.clone()).await?;
                self.add_user_message(content);
                self.input.commit();
            }
        }

        // Regular character
        (KeyCode::Char(c), false) => {
            self.input.insert(c);
        }

        // Backspace
        (KeyCode::Backspace, false) => {
            // Implement backspace
        }

        _ => {}
    }

    Ok(true)
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/app.rs
git commit -m "refactor: rewrite TUI app with new design

- Double Ctrl+C to exit (1s threshold)
- Emacs shortcuts: Ctrl+W/U/K/A/E/P/N
- Ctrl+J for newline
- Tab to toggle folds
- Updated message types with IDs"
```

---

## Task 5: Update Module Exports

**Files:**
- Modify: `crates/tui/src/lib.rs`

- [ ] **Step 1: Export new modules**

```rust
pub mod app;
pub mod fold;
pub mod input;
pub mod markdown;
pub mod render;

pub use app::{App, ChatMessage, Role, ToolCall, MessageId};
pub use fold::{FoldManager, FoldableType};
pub use input::InputBuffer;
```

- [ ] **Step 2: Commit**

```bash
git add crates/tui/src/lib.rs
git commit -m "chore: update TUI module exports"
```

---

## Task 6: Fix Markdown Renderer

**Files:**
- Modify: `crates/tui/src/markdown.rs`

- [ ] **Step 1: Update colors to match new theme**

```rust
// Update color constants in markdown.rs
const CODE_FG: Color = Color::Yellow;
const CODE_BG: Color = Color::Rgb(40, 40, 40);
```

- [ ] **Step 2: Commit**

```bash
git add crates/tui/src/markdown.rs
git commit -m "style: update markdown colors to match new theme"
```

---

## Task 7: Integration Test

- [ ] **Step 1: Run cargo check**

```bash
cargo check -p nekoclaw-tui
```

- [ ] **Step 2: Run tests**

```bash
cargo test -p nekoclaw-tui
```

- [ ] **Step 3: Commit**

```bash
git commit --allow-empty -m "test: TUI redesign complete"
```

---

## Summary

**Changes made:**
- New `input.rs`: Multi-line buffer with Emacs shortcuts
- New `fold.rs`: Collapsible section management
- New `render.rs`: Rendering functions for clean style
- Rewrote `app.rs`: Double Ctrl+C, new key bindings
- Updated `lib.rs`: New module exports
- Updated `markdown.rs`: Color scheme

**Key behaviors:**
- User: `>` prefix, green color
- Assistant: No prefix, white text
- Tools: Collapsible box with border
- Thinking: Collapsible gray text
- Input: Full Emacs-style editing
