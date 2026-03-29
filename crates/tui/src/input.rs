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

    /// Delete character before cursor (Backspace)
    pub fn backspace(&mut self) {
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

        let line = &mut self.lines[self.cursor_line];
        let byte_idx = line.char_indices()
            .nth(self.cursor_col)
            .map(|(i, _)| i)
            .unwrap_or(line.len());

        // Find the previous character's byte position
        let prev_byte_idx = line.char_indices()
            .nth(self.cursor_col - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);

        line.drain(prev_byte_idx..byte_idx);
        self.cursor_col -= 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert() {
        let mut buf = InputBuffer::new();
        buf.insert('h');
        buf.insert('i');
        assert_eq!(buf.content(), "hi");
        assert_eq!(buf.cursor_col(), 2);
    }

    #[test]
    fn test_newline() {
        let mut buf = InputBuffer::new();
        buf.insert('a');
        buf.insert_newline();
        buf.insert('b');
        assert_eq!(buf.content(), "a\nb");
        assert_eq!(buf.cursor_line(), 1);
        assert_eq!(buf.cursor_col(), 1);
    }

    #[test]
    fn test_delete_word() {
        let mut buf = InputBuffer::new();
        buf.insert('h');
        buf.insert('e');
        buf.insert('l');
        buf.insert('l');
        buf.insert('o');
        buf.insert(' ');
        buf.insert('w');
        buf.delete_word();
        assert_eq!(buf.content(), "hello ");
        buf.delete_word();
        assert_eq!(buf.content(), "");
    }

    #[test]
    fn test_delete_to_start() {
        let mut buf = InputBuffer::new();
        buf.insert('h');
        buf.insert('e');
        buf.insert('l');
        buf.insert('l');
        buf.insert('o');
        buf.delete_to_start();
        assert_eq!(buf.content(), "");
        assert_eq!(buf.cursor_col(), 0);
    }

    #[test]
    fn test_delete_to_end() {
        let mut buf = InputBuffer::new();
        buf.insert('h');
        buf.insert('e');
        buf.insert('l');
        buf.insert('l');
        buf.insert('o');
        buf.move_to_start();
        buf.delete_to_end();
        assert_eq!(buf.content(), "");
    }

    #[test]
    fn test_history() {
        let mut buf = InputBuffer::new();
        buf.insert('h');
        buf.insert('i');
        buf.commit();

        buf.insert('b');
        buf.insert('y');
        buf.commit();

        buf.history_prev();
        assert_eq!(buf.content(), "by");

        buf.history_prev();
        assert_eq!(buf.content(), "hi");

        buf.history_next();
        assert_eq!(buf.content(), "by");

        buf.history_next();
        assert_eq!(buf.content(), "");
    }
}
