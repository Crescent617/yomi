use crate::storage::Storage;
use crate::types::{Message, SessionEvent, SessionEventRecord, SessionId, SessionRecord};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
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
    fn rebuild_session(&self, events: &[SessionEventRecord]) -> Option<SessionRecord> {
        let mut session: Option<SessionRecord> = None;
        let mut message_count = 0;

        for record in events {
            match &record.event {
                SessionEvent::Created {
                    session_id,
                    project_path,
                    created_at,
                } => {
                    session = Some(SessionRecord {
                        id: session_id.clone(),
                        project_path: project_path.clone(),
                        created_at: *created_at,
                        updated_at: *created_at,
                        message_count: 0,
                        parent_session_id: None,
                    });
                }
                SessionEvent::MessageAdded { .. } => {
                    message_count += 1;
                }
                SessionEvent::Forked {
                    new_session_id,
                    timestamp,
                    ..
                } => {
                    if let Some(ref mut s) = session {
                        s.parent_session_id = Some(new_session_id.clone());
                        s.updated_at = *timestamp;
                    }
                }
                _ => {}
            }
        }

        session.map(|mut s| {
            s.message_count = message_count;
            s
        })
    }
}

#[async_trait]
impl Storage for FsStorage {
    async fn create_session(&self, project_path: &Path) -> Result<SessionId> {
        let session_id = SessionId::new();
        let event = SessionEvent::created(session_id.clone(), project_path.to_path_buf());
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
        Ok(self.rebuild_session(&events))
    }

    async fn list_sessions(&self, project_path: &Path) -> Result<Vec<SessionRecord>> {
        let mut sessions = Vec::new();
        let mut entries = fs::read_dir(&self.base_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            // Parse session_id from filename
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let session_id = SessionId(stem.to_string());
                let events = self.read_events(&session_id).await?;

                if let Some(session) = self.rebuild_session(&events) {
                    // Only return sessions matching project path
                    if session.project_path == project_path {
                        sessions.push(session);
                    }
                }
            }
        }

        // Sort by created time (newest first)
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(sessions)
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

    async fn update_summary(&self, session_id: &SessionId, summary: &str) -> Result<()> {
        let event = SessionEvent::SummaryUpdated {
            summary: summary.to_string(),
            updated_at: chrono::Utc::now(),
        };
        self.append_event(session_id, event).await
    }

    async fn get_summary(&self, session_id: &SessionId) -> Result<Option<String>> {
        let events = self.read_events(session_id).await?;
        let summary = events
            .into_iter()
            .filter_map(|record| match record.event {
                SessionEvent::SummaryUpdated { summary, .. } => Some(summary),
                _ => None,
            })
            .next_back();
        Ok(summary)
    }
}
