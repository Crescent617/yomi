//! Input editing utilities for text components
//!
//! Provides readline-style editing operations that can be shared between
//! `InputMock`, `CommandPalette`, and any other text input component.

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
    fn kill_to_start_of_line(&mut self) {
        let pos = self.cursor_pos();
        if pos == 0 {
            return;
        }
        let text = self.text();
        let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
        self.text_mut().drain(line_start..pos);
        self.set_cursor_pos(line_start);
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
    fn delete_word_backward(&mut self) {
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
mod tests {
    use super::*;

    #[test]
    fn test_basic_editing() {
        let mut buf = TextBuffer::new();

        buf.insert_str("hello world");
        assert_eq!(buf.content(), "hello world");
        assert_eq!(buf.cursor_pos(), 11);

        buf.move_to_start();
        assert_eq!(buf.cursor_pos(), 0);

        buf.move_word_right();
        assert_eq!(buf.cursor_pos(), 6); // Start of "world" (after "hello ")

        buf.move_word_right();
        assert_eq!(buf.cursor_pos(), 11); // End of "world"
    }

    #[test]
    fn test_delete_word() {
        let mut buf = TextBuffer::with_content("hello world test");
        buf.move_to_end();

        buf.delete_word_backward();
        assert_eq!(buf.content(), "hello world ");

        buf.delete_word_backward();
        assert_eq!(buf.content(), "hello ");
    }

    #[test]
    fn test_kill_line() {
        let mut buf = TextBuffer::with_content("hello\nworld");

        // Position cursor at end of first line (after "hello")
        buf.set_cursor_pos(5);
        assert_eq!(buf.cursor_pos(), 5);

        // Nothing after cursor on first line, so kill_to_end_of_line does nothing
        buf.kill_to_end_of_line();
        assert_eq!(buf.content(), "hello\nworld");

        // Move to start of line and kill - already at start of line, does nothing
        buf.move_to_start_of_line();
        assert_eq!(buf.cursor_pos(), 0);
        buf.kill_to_start_of_line();
        assert_eq!(buf.content(), "hello\nworld");

        // Move to end of first line and kill to start
        buf.move_to_end_of_line();
        assert_eq!(buf.cursor_pos(), 5); // End of "hello"
        buf.kill_to_start_of_line();
        assert_eq!(buf.content(), "\nworld");
    }
}
