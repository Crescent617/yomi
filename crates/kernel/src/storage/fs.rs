use crate::storage::migrations::run_migrations;
use crate::storage::{MetaStorage, SessionInfo, Storage, TokenStorage};
use crate::types::{KernelError, Message, Result, SessionId, SessionRecord};
use async_trait::async_trait;
use sqlx::sqlite::SqlitePool;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

pub struct SimpleStorage {
    base_dir: PathBuf,
    meta: MetaStorage,
    token: TokenStorage,
}

impl SimpleStorage {
    /// Create new `FsStorage` with `SQLite` metadata
    pub async fn new(base_dir: impl Into<PathBuf>, pool: SqlitePool) -> Result<Self> {
        let base_dir = base_dir.into();
        std::fs::create_dir_all(&base_dir).map_err(|e| {
            KernelError::storage(format!("Failed to create storage directory: {e}"))
        })?;

        // Run database migrations before initializing MetaStorage
        run_migrations(&pool)
            .await
            .map_err(|e| KernelError::storage(format!("Failed to run database migrations: {e}")))?;

        let meta = MetaStorage::new(pool.clone());
        let token = TokenStorage::new(pool);

        Ok(Self {
            base_dir,
            meta,
            token,
        })
    }

    /// Get token storage
    pub fn token_storage(&self) -> &TokenStorage {
        &self.token
    }

    /// Default storage path: ~/.local/share/yomi/sessions/
    pub fn default_path() -> PathBuf {
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
            if let Ok(msg) = serde_json::from_str::<Message>(line) {
                messages.push(msg);
            }
        }

        Ok(messages)
    }
}

#[async_trait]
impl Storage for SimpleStorage {
    async fn create_session(&self, working_dir: Option<&str>) -> Result<SessionId> {
        let session_id = SessionId::new();

        // Create metadata in SQLite
        self.meta.create(&session_id, working_dir).await?;

        // Create empty file
        let path = self.session_file_path(&session_id);
        fs::File::create(&path).await?;

        Ok(session_id)
    }

    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId> {
        let parent_path = self.session_file_path(parent_id);
        let new_id = SessionId::new();
        let new_path = self.session_file_path(&new_id);

        // Create metadata first (if this fails, no orphan file)
        self.meta.fork(&new_id, parent_id).await?;

        // Copy file
        if let Err(e) = fs::copy(&parent_path, &new_path).await {
            // Clean up metadata if file copy fails
            let _ = self.meta.delete(&new_id).await;
            return Err(KernelError::storage(format!(
                "Failed to fork session: parent {} not found: {e}",
                parent_id.0
            )));
        }

        // Update message count from copied file
        let messages = self.read_messages(&new_id).await?;
        self.meta
            .update_message_count(&new_id, messages.len() as i64)
            .await?;

        Ok(new_id)
    }

    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>> {
        match self.meta.get(id).await? {
            Some(meta) => Ok(Some(SessionRecord {
                id: meta.id,
                created_at: meta.created_at,
                updated_at: meta.updated_at,
            })),
            None => Ok(None),
        }
    }

    async fn delete_session(&self, id: &SessionId) -> Result<()> {
        // Delete metadata
        self.meta.delete(id).await?;

        // Delete file
        let path = self.session_file_path(id);
        if path.exists() {
            fs::remove_file(&path).await?;
        }
        Ok(())
    }

    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()> {
        use crate::types::Role;

        let path = self.session_file_path(session_id);

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| KernelError::storage(format!("Failed to open session file: {e}")))?;

        // Track last user message for metadata update
        let mut last_user_message: Option<String> = None;

        for message in messages {
            let line = serde_json::to_string(message)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;

            // Track user messages
            if message.role == Role::User {
                let text = message.text_content();
                if !text.is_empty() {
                    last_user_message = Some(text);
                }
            }
        }
        file.flush().await?;

        // Update message count: get existing count and add new messages
        let existing_count = self
            .meta
            .get(session_id)
            .await?
            .map_or(0, |m| m.message_count);
        self.meta
            .update_message_count(session_id, existing_count + messages.len() as i64)
            .await?;

        // Update title with last user message if any (reusing title field for preview)
        if let Some(text) = last_user_message {
            // Truncate to reasonable length for preview (100 chars)
            let truncated = if text.chars().count() > 100 {
                format!("{}...", text.chars().take(100).collect::<String>())
            } else {
                text
            };
            self.meta.update_title(session_id, truncated).await?;
        }

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

        // Update message count
        self.meta
            .update_message_count(session_id, messages.len() as i64)
            .await?;

        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let metas = self.meta.list().await?;

        let sessions = metas
            .into_iter()
            .map(|m| SessionInfo {
                id: m.id.0,
                created_at: m.created_at,
                updated_at: m.updated_at,
                parent_id: m.parent_id.map(|p| p.0),
                title: m.title,
                message_count: m.message_count,
                working_dir: m.working_dir,
            })
            .collect();

        Ok(sessions)
    }

    async fn list_sessions_by_working_dir(&self, working_dir: &str) -> Result<Vec<SessionInfo>> {
        let metas = self.meta.list_by_working_dir(working_dir).await?;

        let sessions = metas
            .into_iter()
            .map(|m| SessionInfo {
                id: m.id.0,
                created_at: m.created_at,
                updated_at: m.updated_at,
                parent_id: m.parent_id.map(|p| p.0),
                title: m.title,
                message_count: m.message_count,
                working_dir: m.working_dir,
            })
            .collect();

        Ok(sessions)
    }

    async fn record_token_usage(&self, record: &crate::types::TokenRecord) -> Result<()> {
        self.token.record(record).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_storage() -> (SimpleStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let storage = SimpleStorage::new(temp_dir.path(), pool).await.unwrap();
        (storage, temp_dir)
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let (storage, _dir) = create_test_storage().await;

        let id = storage.create_session(None).await.unwrap();
        let session = storage.get_session(&id).await.unwrap().unwrap();

        assert_eq!(session.id.0, id.0);
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let (storage, _dir) = create_test_storage().await;

        let id1 = storage.create_session(None).await.unwrap();
        let id2 = storage.create_session(None).await.unwrap();

        // Add message to id1 to make it more recent
        storage
            .append_messages(&id1, &[Message::user("test")])
            .await
            .unwrap();

        let list = storage.list_sessions().await.unwrap();
        assert_eq!(list.len(), 2);
        // id1 should be first (more recent)
        assert_eq!(list[0].id, id1.0);
        assert_eq!(list[1].id, id2.0);
    }

    #[tokio::test]
    async fn test_fork_session() {
        let (storage, _dir) = create_test_storage().await;

        let parent = storage.create_session(Some("/test/dir")).await.unwrap();
        storage
            .append_messages(&parent, &[Message::user("hello")])
            .await
            .unwrap();

        let child = storage.fork_session(&parent).await.unwrap();

        // Check parent relationship
        let list = storage.list_sessions().await.unwrap();
        let child_info = list.iter().find(|s| s.id == child.0).unwrap();
        assert_eq!(child_info.parent_id.as_ref().unwrap(), &parent.0);
        assert_eq!(child_info.message_count, 1);
        // Check working_dir was inherited
        assert_eq!(child_info.working_dir, Some("/test/dir".to_string()));

        // Check messages were copied
        let messages = storage.get_messages(&child).await.unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[tokio::test]
    async fn test_message_count_update() {
        let (storage, _dir) = create_test_storage().await;

        let id = storage.create_session(None).await.unwrap();

        // Add messages
        storage
            .append_messages(&id, &[Message::user("msg1"), Message::assistant("msg2")])
            .await
            .unwrap();

        let list = storage.list_sessions().await.unwrap();
        assert_eq!(list[0].message_count, 2);

        // Set messages (compaction)
        storage
            .set_messages(&id, &[Message::user("compact")])
            .await
            .unwrap();

        let list = storage.list_sessions().await.unwrap();
        assert_eq!(list[0].message_count, 1);
    }
}
