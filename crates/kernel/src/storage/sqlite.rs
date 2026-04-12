use crate::storage::{Storage, StorageConfig};
use crate::types::{Message, SessionId, SessionRecord};
use anyhow::Result;
use async_trait::async_trait;
use sqlx::{migrate::MigrateDatabase, sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::path::Path;

pub struct SqliteStorage {
    pool: Pool<Sqlite>,
    _config: StorageConfig,
}

impl SqliteStorage {
    pub async fn new(config: &StorageConfig) -> Result<Self> {
        let db_url = expand_path(&config.url);
        if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
            Sqlite::create_database(&db_url).await?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await?;
        let storage = Self {
            pool,
            _config: config.clone(),
        };
        storage.migrate().await?;
        Ok(storage)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                project_path TEXT NOT NULL,
                message_count INTEGER DEFAULT 0,
                parent_session_id TEXT,
                summary TEXT
            );
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                tool_calls TEXT,
                tool_call_id TEXT,
                token_usage TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
            ",
        )
        .execute(&self.pool)
        .await?;

        // Migration: add token_usage column if not exists (for existing databases)
        let _ = sqlx::query("ALTER TABLE messages ADD COLUMN token_usage TEXT")
            .execute(&self.pool)
            .await;

        Ok(())
    }
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn create_session(&self, project_path: &Path) -> Result<SessionId> {
        let id = SessionId::new();
        let path_str = project_path.to_string_lossy();
        sqlx::query("INSERT INTO sessions (id, project_path, message_count) VALUES (?, ?, 0)")
            .bind(&id.0)
            .bind(path_str.as_ref())
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId> {
        let new_id = SessionId::new();
        sqlx::query(
            r"INSERT INTO sessions (id, project_path, parent_session_id, message_count)
            SELECT ?, project_path, id, message_count FROM sessions WHERE id = ?",
        )
        .bind(&new_id.0)
        .bind(&parent_id.0)
        .execute(&self.pool)
        .await?;
        sqlx::query(
            r"INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id, created_at)
            SELECT ?, role, content, tool_calls, tool_call_id, created_at
            FROM messages WHERE session_id = ?",
        )
        .bind(&new_id.0)
        .bind(&parent_id.0)
        .execute(&self.pool)
        .await?;
        Ok(new_id)
    }

    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, updated_at, project_path, message_count, parent_session_id
            FROM sessions WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| SessionRecord {
            id: SessionId(r.id),
            created_at: r.created_at,
            updated_at: r.updated_at,
            project_path: r.project_path.into(),
            message_count: r.message_count as usize,
            parent_session_id: r.parent_session_id.map(SessionId),
        }))
    }

    async fn list_sessions(&self, project_path: &Path) -> Result<Vec<SessionRecord>> {
        let path_str = project_path.to_string_lossy();
        let rows = sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, updated_at, project_path, message_count, parent_session_id
            FROM sessions WHERE project_path = ? ORDER BY updated_at DESC",
        )
        .bind(path_str.as_ref())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| SessionRecord {
                id: SessionId(r.id),
                created_at: r.created_at,
                updated_at: r.updated_at,
                project_path: r.project_path.into(),
                message_count: r.message_count as usize,
                parent_session_id: r.parent_session_id.map(SessionId),
            })
            .collect())
    }

    async fn delete_session(&self, id: &SessionId) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()> {
        for msg in messages {
            self.insert_message(&self.pool, session_id, msg).await?;
        }
        sqlx::query(
            "UPDATE sessions SET message_count = (SELECT COUNT(*) FROM messages WHERE session_id = ?),
            updated_at = CURRENT_TIMESTAMP WHERE id = ?"
        ).bind(&session_id.0).bind(&session_id.0).execute(&self.pool).await?;
        Ok(())
    }

    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        let rows = sqlx::query_as::<_, MessageRow>(
            "SELECT role, content, tool_calls, tool_call_id, token_usage, created_at
            FROM messages WHERE session_id = ? ORDER BY id ASC",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let content = if let Ok(blocks) =
                    serde_json::from_str::<Vec<crate::types::ContentBlock>>(&r.content)
                {
                    blocks
                } else {
                    vec![crate::types::ContentBlock::Text { text: r.content }]
                };
                Message {
                    role: parse_role(&r.role),
                    content,
                    tool_calls: r.tool_calls.and_then(|tc| serde_json::from_str(&tc).ok()),
                    tool_call_id: r.tool_call_id,
                    created_at: r.created_at,
                    token_usage: r.token_usage.and_then(|tu| serde_json::from_str(&tu).ok()),
                }
            })
            .collect())
    }

    async fn set_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        // Delete existing messages
        sqlx::query("DELETE FROM messages WHERE session_id = ?")
            .bind(&session_id.0)
            .execute(&mut *tx)
            .await?;
        // Insert new messages
        for msg in messages {
            self.insert_message(&mut *tx, session_id, msg).await?;
        }
        // Update session statistics
        sqlx::query(
            "UPDATE sessions SET message_count = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?"
        )
        .bind(messages.len() as i32)
        .bind(&session_id.0)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

impl SqliteStorage {
    /// Helper to insert a single message into the database
    async fn insert_message(
        &self,
        executor: impl sqlx::Executor<'_, Database = Sqlite>,
        session_id: &SessionId,
        msg: &Message,
    ) -> Result<()> {
        let content = if msg.content.len() == 1 {
            msg.content
                .first()
                .and_then(|c| c.as_text())
                .map(|t| t.to_string())
                .unwrap_or_default()
        } else {
            serde_json::to_string(&msg.content).unwrap_or_default()
        };
        let tool_calls_json = msg
            .tool_calls
            .as_ref()
            .map(|tc| serde_json::to_string(tc).unwrap_or_default());
        let token_usage_json = msg
            .token_usage
            .as_ref()
            .map(|tu| serde_json::to_string(tu).unwrap_or_default());
        sqlx::query(
            "INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id, token_usage)
            VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&session_id.0)
        .bind(format!("{:?}", msg.role).to_lowercase())
        .bind(content)
        .bind(tool_calls_json)
        .bind(&msg.tool_call_id)
        .bind(token_usage_json)
        .execute(executor)
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    project_path: String,
    message_count: i32,
    parent_session_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    role: String,
    content: String,
    tool_calls: Option<String>,
    tool_call_id: Option<String>,
    token_usage: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn expand_path(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return path.replacen('~', &home, 1);
        }
    }
    path.to_string()
}

fn parse_role(role: &str) -> crate::types::Role {
    match role {
        "system" => crate::types::Role::System,
        "assistant" => crate::types::Role::Assistant,
        "tool" => crate::types::Role::Tool,
        _ => crate::types::Role::User,
    }
}
