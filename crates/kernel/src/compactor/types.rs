//! Types for context compaction

use crate::types::Message;

/// Result of a compaction operation
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Summary message (if full compaction was performed)
    pub summary: Option<Message>,
    /// Recent messages that were preserved
    pub keep_messages: Vec<Message>,
    /// Number of messages that were compacted/summarized
    pub compacted_count: usize,
}

impl CompactionResult {
    /// Check if this was a full compaction (has summary)
    pub const fn is_full_compaction(&self) -> bool {
        self.summary.is_some()
    }

    /// Get total message count after compaction
    pub fn total_messages(&self) -> usize {
        self.summary.as_ref().map_or(0, |_| 1) + self.keep_messages.len()
    }
}

/// Statistics about token usage
#[derive(Debug, Clone, Default)]
pub struct TokenStats {
    /// Total tokens before compaction
    pub before_tokens: u32,
    /// Total tokens after compaction
    pub after_tokens: u32,
    /// Number of messages before
    pub before_messages: usize,
    /// Number of messages after
    pub after_messages: usize,
}

impl TokenStats {
    /// Calculate tokens saved
    pub const fn tokens_saved(&self) -> i32 {
        self.before_tokens as i32 - self.after_tokens as i32
    }

    /// Calculate compression ratio
    pub fn compression_ratio(&self) -> f64 {
        if self.before_tokens == 0 {
            return 1.0;
        }
        f64::from(self.after_tokens) / f64::from(self.before_tokens)
    }
}
