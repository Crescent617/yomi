//! Checkpoint V2 - Filesystem-based checkpoint system
//!
//! Complete session snapshot including messages, `file_states`, todos, and file backups.

use crate::types::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod store;
pub use store::FilesystemCheckpointStore;

/// File operation type for tracking
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileOp {
    /// File was created in this turn
    Create,
    /// File was modified in this turn
    Modify,
    /// File was deleted in this turn
    Delete,
}

/// Information about a tracked file
#[derive(Debug, Clone)]
pub struct TrackedFileInfo {
    /// File path
    pub path: PathBuf,
    /// Backup hash (hash of the file content BEFORE modification)
    /// "NULL" for newly created files
    pub backup_hash: String,
    /// Operation type
    pub op: FileOp,
}

/// Checkpoint information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    /// Checkpoint ID: `{message_id}`
    pub id: String,
    /// Session ID
    pub session_id: String,
    /// Message ID that created this checkpoint
    pub message_id: String,
    /// Sequence number (monotonically increasing)
    pub sequence: u32,
    /// Creation timestamp
    pub created_at: u64,
    /// Number of files changed in this checkpoint
    pub files_changed: usize,
    /// User message summary for display
    pub summary: String,
}

/// What to restore during rewind (for backwards compatibility in APIs)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RewindTarget {
    /// Only restore conversation messages
    Conversation,
    /// Only restore files
    Files,
    /// Restore both
    Both,
}

impl RewindTarget {
    pub fn restore_conversation(&self) -> bool {
        matches!(self, Self::Conversation | Self::Both)
    }

    pub fn restore_files(&self) -> bool {
        matches!(self, Self::Files | Self::Both)
    }
}

/// Storage for checkpoints
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// Create a new checkpoint for a completed turn
    ///
    /// - Saves messages.jsonl, `file_states.jsonl`, todos.json to checkpoint directory
    /// - Copies tracked file backups to checkpoint/objects/
    /// - Enforces retention policy
    async fn create_checkpoint(
        &self,
        session_id: &str,
        message_id: &str,
        summary: &str,
        tracked_files: Vec<TrackedFileInfo>,
    ) -> Result<Checkpoint>;

    /// Get all checkpoints for a session (ordered by sequence ascending)
    async fn get_session_checkpoints(&self, session_id: &str) -> Result<Vec<Checkpoint>>;

    /// Get checkpoint by ID within a session
    async fn get_checkpoint(
        &self,
        session_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<Checkpoint>>;

    /// Rewind to a specific checkpoint by sequence number
    ///
    /// Algorithm:
    /// 1. Restore messages.jsonl, `file_states.jsonl`, todos.json from checkpoint
    /// 2. Collect file states from checkpoints after target (newest to target)
    /// 3. Apply collected states to restore files
    /// 4. Delete checkpoints after target
    async fn rewind_to_checkpoint(&self, session_id: &str, target_sequence: u32) -> Result<()>;

    /// Delete a checkpoint within a specific session
    async fn delete_checkpoint(&self, session_id: &str, message_id: &str) -> Result<()>;

    /// Delete all checkpoints for a session
    async fn delete_session_checkpoints(&self, session_id: &str) -> Result<u64>;
}
