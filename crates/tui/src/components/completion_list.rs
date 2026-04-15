//! Generic completion list component for TUI
//!
//! Provides a reusable list structure for command and file completions.

/// Generic completion list for command and file completions
#[derive(Debug)]
pub struct CompletionList<T> {
    visible: bool,
    selected: usize,
    items: Vec<T>,
}

impl<T> Default for CompletionList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> CompletionList<T> {
    /// Create a new empty completion list
    pub fn new() -> Self {
        Self {
            visible: false,
            selected: 0,
            items: Vec::new(),
        }
    }

    /// Check if the completion list is currently visible
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Show the completion list with the given items
    pub fn show(&mut self, items: Vec<T>) {
        self.items = items;
        self.visible = !self.items.is_empty();
        self.selected = 0;
    }

    /// Hide the completion list and clear items
    pub fn hide(&mut self) {
        self.visible = false;
        self.items.clear();
        self.selected = 0;
    }

    /// Move selection to the next item (wraps around)
    pub fn next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    /// Move selection to the previous item (wraps around)
    pub fn prev(&mut self) {
        if !self.items.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.items.len() - 1);
        }
    }

    /// Get the currently selected index
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Get the currently selected item
    pub fn get_selected(&self) -> Option<&T> {
        self.items.get(self.selected)
    }

    /// Get the number of items in the list
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if the list is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get all items in the list
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Get mutable access to items
    pub fn items_mut(&mut self) -> &mut Vec<T> {
        &mut self.items
    }

    /// Set the selected index directly
    ///
    /// # Panics
    /// Panics if index is out of bounds
    pub fn set_selected(&mut self, index: usize) {
        assert!(index < self.items.len(), "index out of bounds");
        self.selected = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_list_basic() {
        let mut list: CompletionList<String> = CompletionList::new();
        assert!(!list.is_visible());
        assert!(list.is_empty());

        list.show(vec!["a".to_string(), "b".to_string()]);
        assert!(list.is_visible());
        assert_eq!(list.len(), 2);

        assert_eq!(list.get_selected(), Some(&"a".to_string()));
        list.next();
        assert_eq!(list.get_selected(), Some(&"b".to_string()));
        list.next();
        assert_eq!(list.get_selected(), Some(&"a".to_string())); // wraps

        list.hide();
        assert!(!list.is_visible());
        assert!(list.is_empty());
    }

    #[test]
    fn test_completion_list_prev() {
        let mut list: CompletionList<i32> = CompletionList::new();
        list.show(vec![1, 2, 3]);

        assert_eq!(list.get_selected(), Some(&1));
        list.prev();
        assert_eq!(list.get_selected(), Some(&3)); // wraps to end
        list.prev();
        assert_eq!(list.get_selected(), Some(&2));
    }

    #[test]
    fn test_empty_list() {
        let mut list: CompletionList<String> = CompletionList::new();
        list.show(vec![]);
        assert!(!list.is_visible()); // empty list doesn't show

        list.next(); // no panic
        list.prev(); // no panic
        assert_eq!(list.get_selected(), None);
    }
}
