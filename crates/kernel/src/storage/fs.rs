use crate::storage::Storage;
use crate::types::{Message, SessionEvent, SessionEventRecord, SessionId, SessionRecord};
use anyhow::{Context, Result};
use async_trait::async_trait;
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

    /// Atomic append write to JSONL
    async fn append_event(&self, session_id: &SessionId, event: SessionEvent) -> Result<()> {
        let record = SessionEventRecord {
            timestamp: chrono::Utc::now(),
            event,
        };

        let line = serde_json::to_string(&record)?;
        let path = self.session_file_path(session_id);

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .context("Failed to open session file")?;

        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        Ok(())
    }

    /// Read all events for a session
    async fn read_events(&self, session_id: &SessionId) -> Result<Vec<SessionEventRecord>> {
        let path = self.session_file_path(session_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&path).await?;
        let mut events = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let record: SessionEventRecord =
                serde_json::from_str(line).context("Failed to parse session event")?;
            events.push(record);
        }

        Ok(events)
    }

    /// Rebuild session record from events
    fn rebuild_session(events: &[SessionEventRecord]) -> Option<SessionRecord> {
        let mut session: Option<SessionRecord> = None;

        for record in events {
            match &record.event {
                SessionEvent::Created {
                    session_id,
                    created_at,
                } => {
                    session = Some(SessionRecord {
                        id: session_id.clone(),
                        created_at: *created_at,
                        updated_at: *created_at,
                    });
                }
                SessionEvent::MessageAdded { timestamp, .. }
                | SessionEvent::Forked { timestamp, .. }
                | SessionEvent::Completed { completed_at: timestamp } => {
                    if let Some(ref mut s) = session {
                        s.updated_at = *timestamp;
                    }
                }
            }
        }

        session
    }
}

#[async_trait]
impl Storage for FsStorage {
    async fn create_session(&self) -> Result<SessionId> {
        let session_id = SessionId::new();
        let event = SessionEvent::created(session_id.clone());
        self.append_event(&session_id, event).await?;
        Ok(session_id)
    }

    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId> {
        let parent_events = self.read_events(parent_id).await?;
        let new_id = SessionId::new();

        // Copy all parent events
        let path = self.session_file_path(&new_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .await?;

        for record in &parent_events {
            let line = serde_json::to_string(record)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        // Add fork event
        let fork_event = SessionEvent::Forked {
            parent_id: parent_id.clone(),
            new_session_id: new_id.clone(),
            timestamp: chrono::Utc::now(),
        };
        self.append_event(&new_id, fork_event).await?;

        Ok(new_id)
    }

    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>> {
        let events = self.read_events(id).await?;
        Ok(Self::rebuild_session(&events))
    }

    async fn delete_session(&self, id: &SessionId) -> Result<()> {
        let path = self.session_file_path(id);
        if path.exists() {
            fs::remove_file(&path).await?;
        }
        Ok(())
    }

    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()> {
        for message in messages {
            let event = SessionEvent::message_added(message.clone());
            self.append_event(session_id, event).await?;
        }
        Ok(())
    }

    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        let events = self.read_events(session_id).await?;
        let messages: Vec<Message> = events
            .into_iter()
            .filter_map(|record| match record.event {
                SessionEvent::MessageAdded { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        Ok(messages)
    }

    async fn set_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()> {
        // Full compaction: replace all messages atomically
        let path = self.session_file_path(session_id);
        let temp_path = path.with_extension("tmp");

        // Get existing events (keep non-message events like Created, Forked, etc.)
        let existing_events = self.read_events(session_id).await?;
        let mut new_events: Vec<SessionEventRecord> = existing_events
            .into_iter()
            .filter(|r| !matches!(r.event, SessionEvent::MessageAdded { .. }))
            .collect();

        // Add new messages
        for message in messages {
            new_events.push(SessionEventRecord {
                timestamp: chrono::Utc::now(),
                event: SessionEvent::message_added(message.clone()),
            });
        }

        // Write to temp file then rename (atomic)
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .await?;

        for record in &new_events {
            let line = serde_json::to_string(record)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
        drop(file);

        // Atomic rename
        fs::rename(&temp_path, &path).await?;

        Ok(())
    }
}
