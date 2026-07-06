//! Input editing utilities for text components
//!
//! Provides readline-style editing operations that can be shared between
//! `InputEditor`, `CommandPalette`, and any other text input component.

/// A trait for components that have editable text content with a cursor position
pub trait TextInput {
    /// Get the current text content
    fn text(&self) -> &str;

    /// Get mutable access to the text content
    fn text_mut(&mut self) -> &mut String;

    /// Get the current cursor position (byte index)
    fn cursor_pos(&self) -> usize;

    /// Set the cursor position
    fn set_cursor_pos(&mut self, pos: usize);

    /// Insert a character at cursor position and advance cursor
    fn insert_char(&mut self, c: char) {
        let pos = self.cursor_pos();
        self.text_mut().insert(pos, c);
        self.set_cursor_pos(pos + c.len_utf8());
    }

    /// Insert a string at cursor position and advance cursor
    fn insert_str(&mut self, s: &str) {
        let pos = self.cursor_pos();
        self.text_mut().insert_str(pos, s);
        self.set_cursor_pos(pos + s.len());
    }

    /// Move cursor left by one character
    fn move_left(&mut self) {
        let pos = self.cursor_pos();
        if pos == 0 {
            return;
        }
        let mut idx = pos - 1;
        let text = self.text();
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        self.set_cursor_pos(idx);
    }

    /// Move cursor right by one character
    fn move_right(&mut self) {
        let pos = self.cursor_pos();
        let text = self.text();
        if pos >= text.len() {
            return;
        }
        let mut idx = pos + 1;
        while idx < text.len() && !text.is_char_boundary(idx) {
            idx += 1;
        }
        self.set_cursor_pos(idx.min(text.len()));
    }

    /// Move cursor to the beginning of the text (Ctrl+Home)
    fn move_to_start(&mut self) {
        self.set_cursor_pos(0);
    }

    /// Move cursor to the end of the text (Ctrl+End)
    fn move_to_end(&mut self) {
        self.set_cursor_pos(self.text().len());
    }

    /// Move cursor to start of current line (Ctrl+A)
    fn move_to_start_of_line(&mut self) {
        let pos = self.cursor_pos();
        let text = self.text();
        let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
        self.set_cursor_pos(line_start);
    }

    /// Move cursor to end of current line (Ctrl+E)
    fn move_to_end_of_line(&mut self) {
        let pos = self.cursor_pos();
        let text = self.text();
        let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
        self.set_cursor_pos(line_end);
    }

    /// Move cursor to previous word boundary (Alt+B)
    fn move_word_left(&mut self) {
        let pos = self.cursor_pos();
        if pos == 0 {
            return;
        }
        let text = self.text();

        // Skip trailing whitespace
        let mut new_pos = pos;
        while new_pos > 0 {
            let mut prev = new_pos - 1;
            while prev > 0 && !text.is_char_boundary(prev) {
                prev -= 1;
            }
            if text[prev..new_pos]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                new_pos = prev;
            } else {
                break;
            }
        }

        // Now find the start of the word
        while new_pos > 0 {
            let mut prev = new_pos - 1;
            while prev > 0 && !text.is_char_boundary(prev) {
                prev -= 1;
            }
            if text[prev..new_pos]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                break;
            }
            new_pos = prev;
        }

        self.set_cursor_pos(new_pos);
    }

    /// Move cursor to next word boundary (Alt+F)
    fn move_word_right(&mut self) {
        let pos = self.cursor_pos();
        let text = self.text();
        if pos >= text.len() {
            return;
        }

        // Skip current word
        let mut new_pos = pos;
        while new_pos < text.len() {
            let mut next = new_pos + 1;
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            if text[new_pos..next]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                break;
            }
            new_pos = next;
        }

        // Now skip whitespace to get to next word
        while new_pos < text.len() {
            let mut next = new_pos + 1;
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            if text[new_pos..next]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                new_pos = next;
            } else {
                break;
            }
        }

        self.set_cursor_pos(new_pos);
    }

    /// Delete character before cursor (Backspace)
    fn backspace(&mut self) {
        let pos = self.cursor_pos();
        if pos == 0 {
            return;
        }
        let text = self.text_mut();
        let mut idx = pos - 1;
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        text.drain(idx..pos);
        self.set_cursor_pos(idx);
    }

    /// Delete character at cursor (Delete key)
    fn delete_char(&mut self) {
        let pos = self.cursor_pos();
        let text = self.text_mut();
        if pos >= text.len() {
            return;
        }
        let mut idx = pos + 1;
        while idx < text.len() && !text.is_char_boundary(idx) {
            idx += 1;
        }
        text.drain(pos..idx);
    }

    /// Delete from cursor to start of line (Ctrl+U)
    /// Falls back to backspace when already at start of line
    fn kill_to_start_of_line(&mut self) {
        let pos = self.cursor_pos();
        if pos == 0 {
            return;
        }
        let text = self.text();
        let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);

        if line_start == pos {
            // Already at start of line, fall back to backspace (delete newline)
            self.backspace();
        } else {
            self.text_mut().drain(line_start..pos);
            self.set_cursor_pos(line_start);
        }
    }

    /// Delete from cursor to end of line (Ctrl+K)
    fn kill_to_end_of_line(&mut self) {
        let pos = self.cursor_pos();
        let text = self.text();
        if pos >= text.len() {
            return;
        }
        let line_end = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
        self.text_mut().drain(pos..line_end);
    }

    /// Delete word backward (Ctrl+W)
    /// Falls back to backspace when already at start of line
    fn delete_word_backward(&mut self) {
        let pos = self.cursor_pos();
        if pos == 0 {
            return;
        }

        let text = self.text();

        // Check if we're at the start of a line (after \n or at position 0)
        let is_at_line_start = pos == 0 || text[..pos].ends_with('\n');

        if is_at_line_start {
            // At start of line, fall back to backspace (delete the newline)
            self.backspace();
            return;
        }

        // Otherwise delete word backward
        let mut new_pos = pos;

        // Skip trailing whitespace
        while new_pos > 0 {
            let mut prev = new_pos - 1;
            while prev > 0 && !text.is_char_boundary(prev) {
                prev -= 1;
            }
            if text[prev..new_pos]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                new_pos = prev;
            } else {
                break;
            }
        }

        // Now find the start of the word
        while new_pos > 0 {
            let mut prev = new_pos - 1;
            while prev > 0 && !text.is_char_boundary(prev) {
                prev -= 1;
            }
            if text[prev..new_pos]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                break;
            }
            new_pos = prev;
        }

        self.text_mut().drain(new_pos..pos);
        self.set_cursor_pos(new_pos);
    }

    /// Delete word forward (Alt+D)
    fn delete_word_forward(&mut self) {
        let pos = self.cursor_pos();
        let text = self.text();
        if pos >= text.len() {
            return;
        }

        // Skip current word
        let mut end_pos = pos;
        while end_pos < text.len() {
            let mut next = end_pos + 1;
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            if text[end_pos..next]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                break;
            }
            end_pos = next;
        }

        // Now skip whitespace
        while end_pos < text.len() {
            let mut next = end_pos + 1;
            while next < text.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            if text[end_pos..next]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                end_pos = next;
            } else {
                break;
            }
        }

        self.text_mut().drain(pos..end_pos);
    }

    /// Clear all text
    fn clear(&mut self) {
        self.text_mut().clear();
        self.set_cursor_pos(0);
    }

    /// Check if the text is empty
    fn is_empty(&self) -> bool {
        self.text().is_empty()
    }
}

/// A simple text input implementation that can be embedded in components
#[derive(Debug, Default, Clone)]
pub struct TextBuffer {
    content: String,
    cursor_pos: usize,
}

impl TextBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_content(content: impl Into<String>) -> Self {
        let content = content.into();
        let len = content.len();
        Self {
            content,
            cursor_pos: len,
        }
    }

    /// Get the content as a string
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Take the content and clear the buffer
    pub fn take(&mut self) -> String {
        let content = std::mem::take(&mut self.content);
        self.cursor_pos = 0;
        content
    }
}

impl TextInput for TextBuffer {
    fn text(&self) -> &str {
        &self.content
    }

    fn text_mut(&mut self) -> &mut String {
        &mut self.content
    }

    fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    fn set_cursor_pos(&mut self, pos: usize) {
        self.cursor_pos = pos.min(self.content.len());
    }
}

#[cfg(test)]
#[path = "input_edit_test.rs"]
mod tests;
