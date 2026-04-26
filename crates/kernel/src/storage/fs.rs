use crate::storage::Storage;
use crate::types::{Message, SessionId, SessionRecord};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::future::join_all;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Filesystem-based storage implementation using JSONL format
pub struct FsStorage {
    base_dir: PathBuf,
}

impl FsStorage {
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_dir = base_dir.into();
        std::fs::create_dir_all(&base_dir).context("Failed to create storage directory")?;
        Ok(Self { base_dir })
    }

    /// Default storage path: ~/.local/share/yomi/sessions/
    pub fn default_path() -> PathBuf {
        // Use ~/.local/share/yomi/sessions as default
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("yomi")
            .join("sessions")
    }

    /// Get session file path
    fn session_file_path(&self, session_id: &SessionId) -> PathBuf {
        self.base_dir.join(format!("{}.jsonl", session_id.0))
    }

    /// Read all messages from session file
    async fn read_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        let path = self.session_file_path(session_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&path).await?;
        let mut messages = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            // Skip lines that fail to parse (e.g., old event types)
            if let Ok(msg) = serde_json::from_str::<Message>(line) {
                messages.push(msg);
            }
        }

        Ok(messages)
    }
}

#[async_trait]
impl Storage for FsStorage {
    async fn create_session(&self) -> Result<SessionId> {
        let session_id = SessionId::new();
        let path = self.session_file_path(&session_id);
        // Create empty file
        fs::File::create(&path).await?;
        Ok(session_id)
    }

    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId> {
        let parent_path = self.session_file_path(parent_id);
        let new_id = SessionId::new();
        let new_path = self.session_file_path(&new_id);

        // Copy file directly
        fs::copy(&parent_path, &new_path)
            .await
            .with_context(|| format!("Failed to fork session: parent {} not found", parent_id.0))?;

        Ok(new_id)
    }

    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>> {
        let path = self.session_file_path(id);
        if !path.exists() {
            return Ok(None);
        }

        let meta = fs::metadata(&path).await?;
        let now = chrono::Utc::now();

        // created() can fail on some filesystems (ext3, tmpfs, etc.)
        let created_at = meta
            .created()
            .ok()
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
            })
            .flatten()
            .unwrap_or(now);

        let updated_at = meta
            .modified()
            .ok()
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
            })
            .flatten()
            .unwrap_or(now);

        Ok(Some(SessionRecord {
            id: id.clone(),
            created_at,
            updated_at,
        }))
    }

    async fn delete_session(&self, id: &SessionId) -> Result<()> {
        let path = self.session_file_path(id);
        if path.exists() {
            fs::remove_file(&path).await?;
        }
        Ok(())
    }

    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()> {
        let path = self.session_file_path(session_id);

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .context("Failed to open session file")?;

        for message in messages {
            let line = serde_json::to_string(message)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;

        Ok(())
    }

    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        self.read_messages(session_id).await
    }

    async fn set_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()> {
        let path = self.session_file_path(session_id);
        let temp_path = path.with_extension("tmp");

        // Write to temp file then rename (atomic)
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .await?;

        for message in messages {
            let line = serde_json::to_string(message)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
        drop(file);

        // Atomic rename
        fs::rename(&temp_path, &path).await?;

        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<crate::storage::SessionInfo>> {
        let mut entries = fs::read_dir(&self.base_dir).await?;
        let mut paths = Vec::new();

        // Collect all jsonl file paths
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                paths.push(path);
            }
        }

        // Concurrently fetch metadata for all files
        let metadata_futs: Vec<_> = paths
            .into_iter()
            .filter_map(|path| {
                let id = path.file_stem().and_then(|s| s.to_str())?.to_string();
                Some(async move {
                    let meta = fs::metadata(&path).await.ok()?;
                    let ctime = meta.created().ok()?;
                    let mtime = meta.modified().ok()?;
                    let created_at = chrono::DateTime::from_timestamp(
                        ctime.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64,
                        0,
                    )?;
                    let updated_at = chrono::DateTime::from_timestamp(
                        mtime.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64,
                        0,
                    )?;
                    Some(crate::storage::SessionInfo {
                        id,
                        created_at,
                        updated_at,
                    })
                })
            })
            .collect();

        let mut sessions: Vec<_> = join_all(metadata_futs)
            .await
            .into_iter()
            .flatten()
            .collect();

        // Sort by updated_at descending (most recent first)
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        Ok(sessions)
    }
}
