//! Unified storage initialization
//!
//! Provides a simple way to initialize all storage backends with a single call.
//! Handles directory creation, database pool setup, migrations, and store instantiation.

use crate::types::{KernelError, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

/// Complete set of storage backends
///
/// This is the primary interface for initializing storage in applications.
/// It handles all setup including database migrations.
#[derive(Clone)]
pub struct StorageSet {
    /// `SQLite` pool shared across `SQLite`-based stores (kept for `Clone`)
    #[allow(dead_code)]
    pool: SqlitePool,
    /// Base directory for file-based storage
    data_dir: PathBuf,
    /// Session metadata store
    session_store: Arc<dyn super::SessionStore>,
    /// Message history store
    message_store: Arc<dyn super::MessageStore>,
    /// Token usage tracking store
    usage_store: Arc<dyn super::UsageStore>,
    /// Todo list persistence
    todo_store: Arc<dyn super::TodoStore>,
}

impl std::fmt::Debug for StorageSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageSet")
            .field("data_dir", &self.data_dir)
            .field("pool", &"<SqlitePool>")
            .field("session_store", &"<dyn SessionStore>")
            .field("message_store", &"<dyn MessageStore>")
            .field("usage_store", &"<dyn UsageStore>")
            .field("todo_store", &"<dyn TodoStore>")
            .finish()
    }
}

impl StorageSet {
    /// Open all storage backends at the given data directory
    ///
    /// This will:
    /// 1. Create the data directory if it doesn't exist
    /// 2. Set up `SQLite` connection pool with proper pragmas
    /// 3. Run database migrations
    /// 4. Initialize all store instances
    ///
    /// # Example
    /// ```no_run
    /// use std::path::PathBuf;
    /// use kernel::storage::StorageSet;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let storage = StorageSet::open(PathBuf::from("~/.yomi")).await?;
    /// // Use the stores...
    /// # Ok(())
    /// # }
    /// ```
    pub async fn open(data_dir: impl Into<PathBuf>) -> Result<Self> {
        let data_dir = data_dir.into();
        let db_path = data_dir.join("yomi.db");
        let sessions_dir = data_dir.join("sessions");

        // Ensure directories exist
        tokio::fs::create_dir_all(&data_dir)
            .await
            .map_err(|e| KernelError::storage(format!("failed to create data dir: {e}")))?;
        tokio::fs::create_dir_all(&sessions_dir)
            .await
            .map_err(|e| KernelError::storage(format!("failed to create sessions dir: {e}")))?;

        // Create SQLite pool with proper settings
        let pool = Self::create_pool(&db_path).await?;

        // Run migrations
        super::migrations::run_migrations(&pool).await?;

        // Create store instances
        let session_store: Arc<dyn super::SessionStore> =
            Arc::new(super::SqliteSessionStore::new(pool.clone()));
        let message_store: Arc<dyn super::MessageStore> =
            Arc::new(super::JsonlMessageStore::new(&sessions_dir));
        let usage_store: Arc<dyn super::UsageStore> =
            Arc::new(super::SqliteUsageStore::new(pool.clone()));
        let todo_store: Arc<dyn super::TodoStore> = Arc::new(super::JsonTodoStore::new(&data_dir));

        Ok(Self {
            pool,
            data_dir,
            session_store,
            message_store,
            usage_store,
            todo_store,
        })
    }

    /// Create `SQLite` pool with recommended settings
    async fn create_pool(db_path: &Path) -> Result<SqlitePool> {
        // Create empty file if it doesn't exist (sqlx requirement)
        if !db_path.exists() {
            tokio::fs::File::create(db_path)
                .await
                .map_err(|e| KernelError::storage(format!("failed to create db file: {e}")))?;
        }

        let connect_options =
            SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))
                .map_err(|e| KernelError::storage(format!("invalid db path: {e}")))?
                .pragma("busy_timeout", "5000")
                .pragma("journal_mode", "WAL");

        SqlitePool::connect_with(connect_options)
            .await
            .map_err(|e| KernelError::storage(format!("failed to connect to db: {e}")))
    }

    /// Get the session store
    pub fn session_store(&self) -> Arc<dyn super::SessionStore> {
        self.session_store.clone()
    }

    /// Get the message store
    pub fn message_store(&self) -> Arc<dyn super::MessageStore> {
        self.message_store.clone()
    }

    /// Get the usage store
    pub fn usage_store(&self) -> Arc<dyn super::UsageStore> {
        self.usage_store.clone()
    }

    /// Get the todo store
    pub fn todo_store(&self) -> Arc<dyn super::TodoStore> {
        self.todo_store.clone()
    }

    /// Get the data directory path
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Get a file state store for a specific session
    ///
    /// File state stores are per-session, so this returns a new instance each time
    pub async fn file_state_store(&self, session_id: &str) -> Result<super::JsonlFileStateStore> {
        super::JsonlFileStateStore::new(session_id, &self.data_dir).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_storage_set_open() {
        let temp_dir = TempDir::new().unwrap();
        let storage = StorageSet::open(temp_dir.path()).await.unwrap();

        // Verify all stores are functional
        let session_id = storage.session_store().create(None).await.unwrap();
        storage
            .message_store()
            .append(&session_id.0, &[])
            .await
            .unwrap();
        storage
            .todo_store()
            .save(&session_id.0, "{}")
            .await
            .unwrap();

        // Verify data directory structure
        assert!(temp_dir.path().join("yomi.db").exists());
        assert!(temp_dir.path().join("sessions").exists());
        assert!(temp_dir.path().join("sessions/todos").exists());
    }

    #[tokio::test]
    async fn test_file_state_store() {
        let temp_dir = TempDir::new().unwrap();
        let storage = StorageSet::open(temp_dir.path()).await.unwrap();

        let file_store = storage.file_state_store("test-session").await.unwrap();
        // Just verify it was created successfully
        drop(file_store);
    }
}
