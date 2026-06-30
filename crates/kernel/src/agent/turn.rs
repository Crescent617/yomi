//! Turn lifecycle management.

use crate::checkpoint::{CheckpointStore, FileOp, RewindTarget, TrackedFileInfo};
use crate::types::{MessageId, Result};
use std::sync::Arc;
use std::sync::Mutex;
use tracing::{debug, info, warn};

/// Tracked file info (internal)
#[derive(Debug, Clone)]
struct TrackedFile {
    pub path: std::path::PathBuf,
    /// Hash of file content BEFORE modification (stored in checkpoint/objects/)
    /// None = file didn't exist (will be created)
    /// Some("SKIPPED") = file too large to backup
    pub hash: Option<String>,
    /// Operation type
    pub op: FileOp,
}

/// Active conversation turn with file tracking and checkpoint management.
///
/// Each turn creates a checkpoint directory at initialization:
/// `{data_dir}/checkpoints/{session_id}/{msg_id}/`
///
/// File backups are stored directly in the checkpoint's objects/ subdirectory.
///
/// Note: `tracked_files` uses `std::sync::Mutex` because all critical sections
/// are brief and never cross an `.await` point (and `Drop` needs sync access).
pub struct Turn {
    /// ID of the user message that started this turn
    pub user_msg_id: MessageId,
    pub session_id: String,
    /// User message summary for checkpoint display
    summary: String,
    tracked_files: Mutex<Vec<TrackedFile>>,
    store: Arc<dyn CheckpointStore>,
    /// Checkpoint directory path
    checkpoint_dir: std::path::PathBuf,
}

impl Turn {
    #[tracing::instrument(skip(summary, store, data_dir, session_id), fields(user_msg_id = %user_msg_id.as_str()))]
    pub fn new(
        user_msg_id: MessageId,
        session_id: impl Into<String>,
        summary: impl Into<String>,
        store: Arc<dyn CheckpointStore>,
        data_dir: &std::path::Path,
    ) -> Self {
        let session_id = session_id.into();
        let checkpoint_dir = data_dir
            .join("checkpoints")
            .join(&session_id)
            .join(user_msg_id.as_str());

        debug!("Turn started -> {:?}", checkpoint_dir);

        // Create checkpoint directory (will be populated during the turn)
        // We don't create it here to allow lazy creation in track_file

        Self {
            user_msg_id,
            session_id,
            summary: summary.into(),
            tracked_files: Mutex::new(Vec::new()),
            store,
            checkpoint_dir,
        }
    }

    /// Get the objects directory for this checkpoint
    fn objects_dir(&self) -> std::path::PathBuf {
        self.checkpoint_dir.join("objects")
    }

    /// Track a file modification - backup current state before modification.
    ///
    /// This should be called BEFORE the file is modified.
    /// Creates a backup of the current content in the checkpoint's objects/ directory.
    pub async fn track_file(&self, path: &std::path::Path) -> Result<()> {
        {
            let files = self.tracked_files.lock().unwrap();
            if files.iter().any(|f| f.path == path) {
                return Ok(());
            }
        }

        // Determine operation type and create backup
        let (hash, op) = match tokio::fs::metadata(path).await {
            Ok(m) if m.is_file() => {
                // File exists - this is a modification
                let hash = self.create_backup(path).await?;
                (hash, FileOp::Modify)
            }
            Ok(_) => {
                // Directory - skip
                return Ok(());
            }
            Err(_) => {
                // File doesn't exist - this is a creation
                (Some("NULL".to_string()), FileOp::Create)
            }
        };

        let mut files = self.tracked_files.lock().unwrap();
        files.push(TrackedFile {
            path: path.to_path_buf(),
            hash,
            op,
        });

        debug!("Tracked file: {} {:?}", path.display(), op);
        Ok(())
    }

    /// Create a backup of a file in the checkpoint's objects directory
    async fn create_backup(&self, path: &std::path::Path) -> Result<Option<String>> {
        const MAX_BACKUP_SIZE: u64 = 10 * 1024 * 1024; // 10MB

        // Check file size
        let metadata = tokio::fs::metadata(path).await.map_err(|e| {
            crate::types::KernelError::io(format!(
                "Failed to read metadata for {}: {}",
                path.display(),
                e
            ))
        })?;

        if metadata.len() > MAX_BACKUP_SIZE {
            warn!(
                "File too large to backup: {} ({} bytes)",
                path.display(),
                metadata.len()
            );
            return Ok(Some("SKIPPED".to_string()));
        }

        // Read content
        let content = tokio::fs::read(path).await.map_err(|e| {
            crate::types::KernelError::io(format!("Failed to read file {}: {}", path.display(), e))
        })?;

        // Calculate hash
        let hash = blake3::hash(&content);
        let hash_str = hash.to_hex()[..16].to_string();

        // Create objects directory
        let objects_dir = self.objects_dir();
        let backup_dir = objects_dir.join(&hash_str[..2]);
        let backup_path = backup_dir.join(&hash_str);

        // Create directory and write backup
        if !backup_path.exists() {
            tokio::fs::create_dir_all(&backup_dir).await.map_err(|e| {
                crate::types::KernelError::io(format!("Failed to create objects directory: {e}"))
            })?;
            tokio::fs::write(&backup_path, content).await.map_err(|e| {
                crate::types::KernelError::io(format!("Failed to write backup: {e}"))
            })?;
            debug!(
                "Created backup: {} -> {}",
                path.display(),
                backup_path.display()
            );
        }

        Ok(Some(hash_str))
    }

    #[tracing::instrument(skip(self))]
    pub async fn complete(&self) -> Result<()> {
        // Convert tracked files to TrackedFileInfo
        let files: Vec<TrackedFile> = {
            let guard = self.tracked_files.lock().unwrap();
            guard.clone()
        };

        let tracked_info: Vec<TrackedFileInfo> = files
            .into_iter()
            .map(|f| TrackedFileInfo {
                path: f.path,
                backup_hash: f.hash.unwrap_or_else(|| "NULL".to_string()),
                op: f.op,
            })
            .collect();

        // Create checkpoint with summary
        let cp = self
            .store
            .create_checkpoint(
                &self.session_id,
                self.user_msg_id.as_str(),
                &self.summary,
                tracked_info,
            )
            .await?;

        info!(
            "Turn completed (checkpoint seq={}, {} files, summary: {})",
            cp.sequence, cp.files_changed, cp.summary
        );

        Ok(())
    }

    /// Cancel turn - discard tracked files and cleanup checkpoint directory.
    ///
    /// This removes any file backups that were created during this turn.
    #[tracing::instrument(skip(self))]
    pub async fn cancel(&self) -> Result<()> {
        let files = self.take_tracked_files();

        // Remove checkpoint directory if it exists (cleanup orphaned backups)
        if self.checkpoint_dir.exists() {
            tokio::fs::remove_dir_all(&self.checkpoint_dir)
                .await
                .map_err(|e| {
                    crate::types::KernelError::io(format!(
                        "Failed to cleanup checkpoint directory: {e}"
                    ))
                })?;
        }

        debug!(
            "Turn cancelled ({} files tracked, directory cleaned)",
            files.len()
        );
        Ok(())
    }

    fn take_tracked_files(&self) -> Vec<TrackedFile> {
        let mut files = self.tracked_files.lock().unwrap();
        std::mem::take(&mut *files)
    }

    /// Rewind to a checkpoint.
    ///
    /// This delegates to the `CheckpointStore` implementation.
    /// Returns the number of checkpoints deleted.
    #[tracing::instrument(skip(store))]
    pub async fn rewind_to_checkpoint(
        session_id: &str,
        target_message_id: &MessageId,
        target: RewindTarget,
        store: &Arc<dyn CheckpointStore>,
    ) -> Result<usize> {
        let checkpoints = store.get_session_checkpoints(session_id).await?;
        let target_cp = checkpoints
            .iter()
            .find(|c| c.message_id == target_message_id.as_str())
            .ok_or_else(|| {
                crate::types::KernelError::storage(format!(
                    "Checkpoint {} not found",
                    target_message_id.as_str()
                ))
            })?;

        if !target.restore_files() {
            // Just delete target and later checkpoints
            let to_delete: Vec<_> = checkpoints
                .iter()
                .filter(|c| c.sequence >= target_cp.sequence)
                .map(|c| c.message_id.clone())
                .collect();

            let mut deleted = 0;
            for msg_id in to_delete {
                if let Err(e) = store.delete_checkpoint(session_id, &msg_id).await {
                    warn!("Failed to delete checkpoint {}: {}", msg_id, e);
                } else {
                    deleted += 1;
                }
            }
            return Ok(deleted);
        }

        // Call store's rewind method
        store
            .rewind_to_checkpoint(session_id, target_cp.sequence)
            .await?;

        // Count checkpoints deleted
        let remaining = store.get_session_checkpoints(session_id).await?;
        let deleted_count = checkpoints.len() - remaining.len();

        info!(
            "Rewind completed to seq={} ({} checkpoints deleted)",
            target_cp.sequence, deleted_count
        );

        Ok(deleted_count)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let files = self.take_tracked_files();
        if !files.is_empty() {
            debug!("Turn dropped with {} tracked files", files.len(),);
        }
        // NOTE: We intentionally do NOT delete the checkpoint directory here.
        // The directory holds file backups (in objects/) that are referenced by
        // the CheckpointStore. Those backups must survive until the user
        // explicitly rewinds / deletes the checkpoint or the session is
        // cleaned up. Deleting on Drop would break the undo feature.
    }
}
