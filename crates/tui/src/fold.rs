use crate::model::MessageId;

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
    pub const fn new() -> Self {
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
        self.get(id).is_some_and(|f| f.is_expanded)
    }

    /// Navigate to next fold (for Tab navigation)
    pub const fn next_fold(&mut self) {
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
                    .map(|n| format!("({n} tokens)"))
                    .unwrap_or_default();
                format!("{icon} Thinking {tokens}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_toggle() {
        let mut fm = FoldManager::new();
        fm.register(1, FoldableType::Tools, "Tools (2)".to_string(), None);

        assert!(!fm.is_expanded(1));
        fm.toggle_by_id(1);
        assert!(fm.is_expanded(1));
    }

    #[test]
    fn test_focus_navigation() {
        let mut fm = FoldManager::new();
        fm.register(1, FoldableType::Tools, "Tools (1)".to_string(), None);
        fm.register(2, FoldableType::Thinking, "".to_string(), Some(100));

        assert_eq!(fm.focused_id(), None);
        fm.next_fold();
        assert_eq!(fm.focused_id(), Some(1));
        fm.next_fold();
        assert_eq!(fm.focused_id(), Some(2));
        fm.next_fold();
        assert_eq!(fm.focused_id(), Some(1)); // Wrap around
    }

    #[test]
    fn test_render_indicator() {
        let fold = FoldState {
            id: 1,
            fold_type: FoldableType::Tools,
            is_expanded: false,
            summary: "Tools (2)".to_string(),
            token_count: None,
        };
        assert_eq!(FoldManager::render_indicator(&fold), "▶ Tools (2)");

        let fold2 = FoldState {
            id: 2,
            fold_type: FoldableType::Thinking,
            is_expanded: true,
            summary: String::new(),
            token_count: Some(256),
        };
        assert_eq!(FoldManager::render_indicator(&fold2), "▼ Thinking (256 tokens)");
    }
}
