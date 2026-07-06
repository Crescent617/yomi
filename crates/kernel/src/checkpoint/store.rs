//! Checkpoint V2 - Filesystem-based checkpoint system
//!
//! Complete session snapshot including messages, `file_states`, todos, and file backups.
//! Each checkpoint is self-contained in its own directory for atomic cleanup.

use crate::types::{KernelError, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};

/// Checkpoint metadata stored in manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointInfo {
    pub timestamp: u64,
    pub message_id: String,
    pub sequence: u32,
    /// User message summary for display in rewind picker
    pub summary: String,
}

/// Manifest for a session's checkpoints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub session_id: String,
    pub checkpoints: Vec<CheckpointInfo>,
    #[serde(skip)]
    next_sequence: u32,
}

impl Manifest {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            checkpoints: Vec::new(),
            next_sequence: 1,
        }
    }

    pub fn next_sequence(&mut self) -> u32 {
        let seq = self.next_sequence;
        self.next_sequence += 1;
        seq
    }

    pub fn add(&mut self, info: CheckpointInfo) {
        self.checkpoints.push(info);
        // Ensure sorted by sequence
        self.checkpoints.sort_by_key(|c| c.sequence);
    }

    pub fn remove(&mut self, message_id: &str) {
        self.checkpoints.retain(|c| c.message_id != message_id);
    }

    pub fn get_by_sequence(&self, sequence: u32) -> Option<&CheckpointInfo> {
        self.checkpoints.iter().find(|c| c.sequence == sequence)
    }

    pub fn get_by_message_id(&self, message_id: &str) -> Option<&CheckpointInfo> {
        self.checkpoints.iter().find(|c| c.message_id == message_id)
    }
}

/// File info stored in checkpoint meta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: PathBuf,
    pub hash: String, // "NULL" for newly created files
    pub op: crate::checkpoint::FileOp,
}

/// Checkpoint metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMeta {
    pub session_id: String,
    pub timestamp: u64,
    pub message_id: String,
    pub sequence: u32,
    pub files: Vec<FileInfo>,
}

/// Filesystem-based checkpoint store
pub struct FilesystemCheckpointStore {
    base_dir: PathBuf,
    max_checkpoints: usize,
}

impl FilesystemCheckpointStore {
    /// Create new store with the given data directory and default `max_checkpoints` (5)
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: data_dir.into().join("checkpoints"),
            max_checkpoints: 5,
        }
    }

    /// Create new store with specified `max_checkpoints`
    pub fn with_max_checkpoints(data_dir: impl Into<PathBuf>, max: usize) -> Self {
        Self {
            base_dir: data_dir.into().join("checkpoints"),
            max_checkpoints: max,
        }
    }

    /// Get session directory
    fn session_dir(&self, session_id: &str) -> PathBuf {
        // Sanitize session_id to prevent path traversal
        let safe_id = session_id.replace(['/', '\\'], "_");
        self.base_dir.join(&safe_id)
    }

    /// Get checkpoint directory (`checkpoint_id` = `message_id`)
    pub fn checkpoint_dir(&self, session_id: &str, message_id: &str) -> PathBuf {
        self.session_dir(session_id).join(message_id)
    }

    /// Get objects directory for a checkpoint
    pub fn objects_dir(&self, session_id: &str, message_id: &str) -> PathBuf {
        self.checkpoint_dir(session_id, message_id).join("objects")
    }

    /// Get manifest path
    fn manifest_path(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("manifest.json")
    }

    /// Load or create manifest
    async fn load_manifest(&self, session_id: &str) -> Result<Manifest> {
        let path = self.manifest_path(session_id);
        if path.exists() {
            let content = fs::read_to_string(&path)
                .await
                .map_err(|e| KernelError::io(format!("Failed to read manifest: {e}")))?;
            let mut manifest: Manifest = serde_json::from_str(&content)
                .map_err(|e| KernelError::io(format!("Failed to parse manifest: {e}")))?;
            // Restore next_sequence
            manifest.next_sequence = manifest
                .checkpoints
                .iter()
                .map(|c| c.sequence)
                .max()
                .unwrap_or(0)
                + 1;
            Ok(manifest)
        } else {
            Ok(Manifest::new(session_id.to_string()))
        }
    }

    /// Save manifest
    async fn save_manifest(&self, manifest: &Manifest) -> Result<()> {
        let path = self.manifest_path(&manifest.session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| KernelError::io(format!("Failed to create directory: {e}")))?;
        }
        let content = serde_json::to_string_pretty(manifest)
            .map_err(|e| KernelError::io(format!("Failed to serialize manifest: {e}")))?;
        fs::write(&path, content)
            .await
            .map_err(|e| KernelError::io(format!("Failed to write manifest: {e}")))?;
        Ok(())
    }

    /// Enforce retention policy - remove oldest checkpoints if over limit
    async fn enforce_retention(&self, manifest: &mut Manifest) -> Result<()> {
        while manifest.checkpoints.len() > self.max_checkpoints {
            let oldest = manifest.checkpoints.first().cloned();
            if let Some(cp) = oldest {
                let session_id = manifest.session_id.clone();
                self.delete_checkpoint_internal(&session_id, &cp.message_id, manifest)
                    .await?;
            } else {
                break;
            }
        }
        Ok(())
    }

    /// Internal delete that updates manifest
    async fn delete_checkpoint_internal(
        &self,
        session_id: &str,
        checkpoint_id: &str,
        manifest: &mut Manifest,
    ) -> Result<()> {
        let dir = self.checkpoint_dir(session_id, checkpoint_id);
        if dir.exists() {
            fs::remove_dir_all(&dir).await.map_err(|e| {
                KernelError::io(format!("Failed to delete checkpoint directory: {e}"))
            })?;
            info!("Deleted checkpoint: {}", checkpoint_id);
        }
        manifest.remove(checkpoint_id);
        Ok(())
    }

    /// Copy file helper
    async fn copy_file(&self, from: &Path, to: &Path) -> Result<()> {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| KernelError::io(format!("Failed to create directory: {e}")))?;
        }
        fs::copy(from, to).await.map_err(|e| {
            KernelError::io(format!(
                "Failed to copy file from {} to {}: {e}",
                from.display(),
                to.display()
            ))
        })?;
        Ok(())
    }
}

#[async_trait]
impl crate::checkpoint::CheckpointStore for FilesystemCheckpointStore {
    async fn create_checkpoint(
        &self,
        session_id: &str,
        message_id: &str,
        summary: &str,
        tracked_files: Vec<crate::checkpoint::TrackedFileInfo>,
    ) -> Result<crate::checkpoint::Checkpoint> {
        let mut manifest = self.load_manifest(session_id).await?;
        let seq = manifest.next_sequence();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let dir = self.checkpoint_dir(session_id, message_id);

        // Ensure checkpoint directory exists
        fs::create_dir_all(&dir)
            .await
            .map_err(|e| KernelError::io(format!("Failed to create checkpoint directory: {e}")))?;

        // Get paths to session files
        let sessions_dir = self.base_dir.parent().unwrap().join("sessions");
        let messages_path = sessions_dir.join(format!("{session_id}.jsonl"));
        let file_states_path = sessions_dir
            .join("file_states")
            .join(format!("{session_id}.jsonl"));
        let todos_path = sessions_dir
            .join("todos")
            .join(format!("{session_id}.json"));

        // Copy messages.jsonl
        if messages_path.exists() {
            self.copy_file(&messages_path, &dir.join("messages.jsonl"))
                .await?;
        }

        // Copy file_states.jsonl if exists
        if file_states_path.exists() {
            self.copy_file(&file_states_path, &dir.join("file_states.jsonl"))
                .await?;
        }

        // Copy todos.json if exists
        if todos_path.exists() {
            self.copy_file(&todos_path, &dir.join("todos.json")).await?;
        }

        // Build file info from tracked files (backups already in objects/ from track_file)
        let files: Vec<FileInfo> = tracked_files
            .into_iter()
            .map(|tracked| FileInfo {
                path: tracked.path,
                hash: tracked.backup_hash,
                op: tracked.op,
            })
            .collect();

        // Write meta.json
        let files_changed = files.len();
        let meta = CheckpointMeta {
            session_id: session_id.to_string(),
            timestamp,
            message_id: message_id.to_string(),
            sequence: seq,
            files,
        };
        let meta_content = serde_json::to_string_pretty(&meta)
            .map_err(|e| KernelError::io(format!("Failed to serialize meta: {e}")))?;
        fs::write(dir.join("meta.json"), meta_content)
            .await
            .map_err(|e| KernelError::io(format!("Failed to write meta: {e}")))?;

        // Enforce retention BEFORE adding new checkpoint
        // This ensures we never create a checkpoint that's immediately deleted
        self.enforce_retention(&mut manifest).await?;

        // Update manifest with new checkpoint
        manifest.add(CheckpointInfo {
            timestamp,
            message_id: message_id.to_string(),
            sequence: seq,
            summary: summary.to_string(),
        });
        self.save_manifest(&manifest).await?;

        debug!(
            "Created checkpoint: {} for session {} (seq={})",
            message_id, session_id, seq
        );

        Ok(crate::checkpoint::Checkpoint {
            id: message_id.to_string(),
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            sequence: seq,
            created_at: timestamp,
            files_changed,
            summary: summary.to_string(),
        })
    }

    async fn get_session_checkpoints(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        let manifest = self.load_manifest(session_id).await?;
        let mut checkpoints = Vec::new();

        for info in manifest.checkpoints {
            // Try to read files_changed from meta
            let meta_path = self
                .checkpoint_dir(session_id, &info.message_id)
                .join("meta.json");
            let files_changed = if meta_path.exists() {
                fs::read_to_string(&meta_path)
                    .await
                    .ok()
                    .and_then(|s| serde_json::from_str::<CheckpointMeta>(&s).ok())
                    .map_or(0, |m| m.files.len())
            } else {
                0
            };

            checkpoints.push(crate::checkpoint::Checkpoint {
                id: info.message_id.clone(),
                session_id: session_id.to_string(),
                message_id: info.message_id,
                sequence: info.sequence,
                created_at: info.timestamp,
                files_changed,
                summary: info.summary,
            });
        }

        Ok(checkpoints)
    }

    async fn get_checkpoint(
        &self,
        session_id: &str,
        checkpoint_id: &str,
    ) -> Result<Option<crate::checkpoint::Checkpoint>> {
        let manifest = self.load_manifest(session_id).await?;

        // Read files_changed from meta
        let meta_path = self
            .checkpoint_dir(session_id, checkpoint_id)
            .join("meta.json");
        let files_changed = if meta_path.exists() {
            fs::read_to_string(&meta_path)
                .await
                .ok()
                .and_then(|s| serde_json::from_str::<CheckpointMeta>(&s).ok())
                .map_or(0, |m| m.files.len())
        } else {
            0
        };

        Ok(manifest
            .get_by_message_id(checkpoint_id)
            .map(|info| crate::checkpoint::Checkpoint {
                id: info.message_id.clone(),
                session_id: session_id.to_string(),
                message_id: info.message_id.clone(),
                sequence: info.sequence,
                created_at: info.timestamp,
                files_changed,
                summary: info.summary.clone(),
            }))
    }

    async fn rewind_to_checkpoint(&self, session_id: &str, target_sequence: u32) -> Result<()> {
        let mut manifest = self.load_manifest(session_id).await?;
        let target_cp = manifest
            .get_by_sequence(target_sequence)
            .cloned()
            .ok_or_else(|| {
                KernelError::storage(format!("Checkpoint seq={target_sequence} not found"))
            })?;

        let target_dir = self.checkpoint_dir(session_id, &target_cp.message_id);

        // Get paths to session files
        let sessions_dir = self.base_dir.parent().unwrap().join("sessions");
        let messages_path = sessions_dir.join(format!("{session_id}.jsonl"));
        let file_states_path = sessions_dir
            .join("file_states")
            .join(format!("{session_id}.jsonl"));
        let todos_path = sessions_dir
            .join("todos")
            .join(format!("{session_id}.json"));

        // 1. Restore messages.jsonl
        let src = target_dir.join("messages.jsonl");
        if src.exists() {
            self.copy_file(&src, &messages_path).await?;
            info!("Restored messages for checkpoint seq={}", target_sequence);
        }

        // 2. Restore file_states.jsonl if exists
        let src = target_dir.join("file_states.jsonl");
        if src.exists() {
            self.copy_file(&src, &file_states_path).await?;
        }

        // 3. Restore todos.json if exists
        let src = target_dir.join("todos.json");
        if src.exists() {
            self.copy_file(&src, &todos_path).await?;
        }

        // 4. Restore files - collect states from target checkpoint and all after it
        // We need to know what files existed at the target checkpoint time
        let mut file_states: HashMap<PathBuf, FileInfo> = HashMap::new();

        // First, read target checkpoint's meta to get files that existed at that point
        let target_meta_path = target_dir.join("meta.json");
        if target_meta_path.exists() {
            let content = fs::read_to_string(&target_meta_path)
                .await
                .map_err(|e| KernelError::io(format!("Failed to read target meta: {e}")))?;
            let target_meta: CheckpointMeta = serde_json::from_str(&content)
                .map_err(|e| KernelError::io(format!("Failed to parse target meta: {e}")))?;

            // For target checkpoint:
            // - Create files (hash=NULL): they didn't exist before, so at target they are newly created
            // - Modify files (hash=backup): they existed with the backup content
            for file in target_meta.files {
                file_states.insert(file.path.clone(), file);
            }
        }

        // Then, iterate from newest to target+1 to override with later changes
        for info in manifest.checkpoints.iter().rev() {
            if info.sequence <= target_sequence {
                break;
            }

            let meta_path = self
                .checkpoint_dir(session_id, &info.message_id)
                .join("meta.json");
            if !meta_path.exists() {
                continue;
            }

            let content = fs::read_to_string(&meta_path)
                .await
                .map_err(|e| KernelError::io(format!("Failed to read meta: {e}")))?;
            let meta: CheckpointMeta = serde_json::from_str(&content)
                .map_err(|e| KernelError::io(format!("Failed to parse meta: {e}")))?;

            for file in meta.files {
                // Later changes override earlier ones
                file_states.entry(file.path.clone()).or_insert(file);
            }
        }

        // Apply collected file states
        for (path, info) in file_states {
            match info.op {
                crate::checkpoint::FileOp::Create if info.hash == "NULL" => {
                    // File was created at this checkpoint
                    // If it's from target checkpoint, it should exist (keep it)
                    // If it's from after target, it should be deleted
                    // We determine this by checking if the file exists in target's objects
                    let in_target = self
                        .checkpoint_dir(session_id, &target_cp.message_id)
                        .join("objects")
                        .join(&info.hash[..2])
                        .join(&info.hash)
                        .exists();

                    if !in_target && path.exists() {
                        fs::remove_file(&path).await.map_err(|e| {
                            KernelError::io(format!(
                                "Failed to remove file {}: {e}",
                                path.display()
                            ))
                        })?;
                        info!(
                            "Deleted file (was created after target): {}",
                            path.display()
                        );
                    }
                }
                crate::checkpoint::FileOp::Delete => {
                    // File was deleted at this checkpoint
                    // TODO: Need to restore from an earlier checkpoint
                    warn!(
                        "File deletion restore not yet implemented for: {}",
                        path.display()
                    );
                }
                _ => {
                    // Restore the backup (hash points to the pre-modification state)
                    // Try target checkpoint first, then later ones
                    let mut restored = false;

                    // Check target checkpoint
                    let src = target_dir
                        .join("objects")
                        .join(&info.hash[..2])
                        .join(&info.hash);
                    if src.exists() {
                        self.copy_file(&src, &path).await?;
                        info!(
                            "Restored file {} from target checkpoint: hash={}",
                            path.display(),
                            info.hash
                        );
                        restored = true;
                    } else {
                        // Check later checkpoints
                        for cp_info in manifest.checkpoints.iter().rev() {
                            if cp_info.sequence <= target_sequence {
                                break;
                            }
                            let src = self
                                .checkpoint_dir(session_id, &cp_info.message_id)
                                .join("objects")
                                .join(&info.hash[..2])
                                .join(&info.hash);
                            if src.exists() {
                                self.copy_file(&src, &path).await?;
                                info!(
                                    "Restored file {} from checkpoint {}: hash={}",
                                    path.display(),
                                    cp_info.message_id,
                                    info.hash
                                );
                                restored = true;
                                break;
                            }
                        }
                    }

                    if !restored {
                        warn!(
                            "Backup not found for file {}: hash={}",
                            path.display(),
                            info.hash
                        );
                    }
                }
            }
        }

        // 5. Delete target checkpoint and all after it (including their objects directories)
        // Note: We delete after restoring files, so the backups in objects/ are available
        // during restoration
        let to_delete: Vec<_> = manifest
            .checkpoints
            .iter()
            .filter(|c| c.sequence >= target_sequence) // Changed: >= instead of >
            .map(|c| c.message_id.clone())
            .collect();

        for msg_id in to_delete {
            let dir = self.checkpoint_dir(session_id, &msg_id);
            if dir.exists() {
                fs::remove_dir_all(&dir).await.map_err(|e| {
                    KernelError::io(format!("Failed to delete checkpoint directory: {e}"))
                })?;
            }
            manifest.remove(&msg_id);
        }

        // 6. Save manifest
        self.save_manifest(&manifest).await?;

        info!(
            "Rewound session {} to checkpoint seq={}",
            session_id, target_sequence
        );
        Ok(())
    }

    async fn delete_checkpoint(&self, session_id: &str, message_id: &str) -> Result<()> {
        let mut manifest = self.load_manifest(session_id).await?;

        if manifest.get_by_message_id(message_id).is_none() {
            return Err(KernelError::storage(format!(
                "Checkpoint {message_id} not found in session {session_id}"
            )));
        }

        self.delete_checkpoint_internal(session_id, message_id, &mut manifest)
            .await?;
        self.save_manifest(&manifest).await?;
        Ok(())
    }

    async fn delete_session_checkpoints(&self, session_id: &str) -> Result<u64> {
        let dir = self.session_dir(session_id);
        if !dir.exists() {
            return Ok(0);
        }

        let entries = fs::read_dir(&dir)
            .await
            .map_err(|e| KernelError::io(format!("Failed to read session directory: {e}")))?;

        let mut count = 0u64;
        let mut entries = entries;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| KernelError::io(format!("Failed to read directory entry: {e}")))?
        {
            let path = entry.path();
            if path.is_dir() && path.file_name() != Some("manifest.json".as_ref()) {
                fs::remove_dir_all(&path).await.map_err(|e| {
                    KernelError::io(format!("Failed to remove checkpoint directory: {e}"))
                })?;
                count += 1;
            }
        }

        // Remove manifest
        let manifest_path = self.manifest_path(session_id);
        if manifest_path.exists() {
            fs::remove_file(&manifest_path)
                .await
                .map_err(|e| KernelError::io(format!("Failed to remove manifest: {e}")))?;
        }

        // Try to remove session directory
        let _ = fs::remove_dir(&dir).await;

        info!("Deleted {} checkpoints for session {}", count, session_id);
        Ok(count)
    }

    async fn copy_session_checkpoints(
        &self,
        from_session_id: &str,
        to_session_id: &str,
    ) -> Result<u64> {
        let from_dir = self.session_dir(from_session_id);
        if !from_dir.exists() {
            return Ok(0);
        }

        let to_dir = self.session_dir(to_session_id);
        let mut count = 0u64;

        // Copy manifest
        let from_manifest = self.manifest_path(from_session_id);
        if from_manifest.exists() {
            let to_manifest = self.manifest_path(to_session_id);
            if let Some(parent) = to_manifest.parent() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    KernelError::io(format!("Failed to create session directory: {e}"))
                })?;
            }
            if !to_manifest.exists() {
                fs::copy(&from_manifest, &to_manifest)
                    .await
                    .map_err(|e| KernelError::io(format!("Failed to copy manifest: {e}")))?;
                count += 1;
            }
        }

        // Copy checkpoint directories
        let entries = fs::read_dir(&from_dir)
            .await
            .map_err(|e| KernelError::io(format!("Failed to read session directory: {e}")))?;
        let mut entries = entries;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| KernelError::io(format!("Failed to read directory entry: {e}")))?
        {
            let path = entry.path();
            if path.is_dir() {
                let to_path = to_dir.join(path.file_name().unwrap_or_default());
                if !to_path.exists() {
                    Self::copy_dir_recursive(&path, &to_path).await?;
                    count += 1;
                }
            }
        }

        info!(
            "Copied {} checkpoints from {} to {}",
            count, from_session_id, to_session_id
        );
        Ok(count)
    }
}

impl FilesystemCheckpointStore {
    async fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
        fs::create_dir_all(to)
            .await
            .map_err(|e| KernelError::io(format!("Failed to create directory: {e}")))?;

        let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];

        while let Some((src_dir, dst_dir)) = stack.pop() {
            let mut entries = fs::read_dir(&src_dir)
                .await
                .map_err(|e| KernelError::io(format!("Failed to read directory: {e}")))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| KernelError::io(format!("Failed to read directory entry: {e}")))?
            {
                let path = entry.path();
                let to_path = dst_dir.join(path.file_name().unwrap_or_default());
                if path.is_dir() {
                    fs::create_dir_all(&to_path)
                        .await
                        .map_err(|e| KernelError::io(format!("Failed to create directory: {e}")))?;
                    stack.push((path, to_path));
                } else if !to_path.exists() {
                    fs::copy(&path, &to_path)
                        .await
                        .map_err(|e| KernelError::io(format!("Failed to copy file: {e}")))?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "store_test.rs"]
mod tests;
