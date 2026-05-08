//! History navigation for the input component

use super::component::InputComponent;
use super::editor::InputEditor;

impl InputComponent {
    /// Set the history entries
    pub fn set_history(&mut self, history: Vec<String>) {
        self.history = history;
        self.history_index = None;
        self.saved_input = String::new();
    }

    /// Navigate to previous history entry (Ctrl+P)
    pub(crate) fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }

        match self.history_index {
            None => {
                // Save current input and go to last history entry
                self.saved_input = self.component.content().to_string();
                let last_idx = self.history.len() - 1;
                self.component = InputEditor::new();
                self.component.insert_str(&self.history[last_idx]);
                self.history_index = Some(last_idx);
            }
            Some(idx) if idx > 0 => {
                // Go to older entry
                let new_idx = idx - 1;
                self.component = InputEditor::new();
                self.component.insert_str(&self.history[new_idx]);
                self.history_index = Some(new_idx);
            }
            Some(_) => {
                // Already at oldest
            }
        }
    }

    /// Navigate to next history entry (Ctrl+N)
    pub(crate) fn history_next(&mut self) {
        match self.history_index {
            None => {
                // Already at newest (editing new input)
            }
            Some(idx) if idx + 1 < self.history.len() => {
                // Go to newer entry
                let new_idx = idx + 1;
                self.component = InputEditor::new();
                self.component.insert_str(&self.history[new_idx]);
                self.history_index = Some(new_idx);
            }
            Some(_) => {
                // Return to saved input
                self.component = InputEditor::new();
                self.component.insert_str(&self.saved_input);
                self.history_index = None;
            }
        }
    }
}
