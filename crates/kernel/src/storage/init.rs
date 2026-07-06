//! Unified storage initialization
//!
//! Provides a simple way to initialize all storage backends with a single call.
//! Handles directory creation, database pool setup, migrations, and store instantiation.

use crate::cron::SqliteCronStore;
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
    /// Goal state persistence
    goal_store: Arc<dyn crate::goal::GoalStore>,
    /// Todo list persistence
    todo_store: Arc<dyn super::TodoStore>,
    /// Checkpoint and file history store
    checkpoint_store: Arc<dyn crate::checkpoint::CheckpointStore>,
    /// Project metadata store
    project_store: Arc<dyn super::ProjectStore>,
    /// Pinned session metadata store
    pinned_session_store: Arc<dyn super::PinnedSessionStore>,
    /// Cron job store
    cron_store: Arc<dyn crate::cron::CronStore>,
    /// Channel session mapping store
    channel_store: Arc<dyn crate::channels::ChannelStore>,
}

impl std::fmt::Debug for StorageSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageSet")
            .field("data_dir", &self.data_dir)
            .field("pool", &"<SqlitePool>")
            .field("session_store", &"<dyn SessionStore>")
            .field("message_store", &"<dyn MessageStore>")
            .field("usage_store", &"<dyn UsageStore>")
            .field("goal_store", &"<dyn GoalStore>")
            .field("todo_store", &"<dyn TodoStore>")
            .field("checkpoint_store", &"<dyn CheckpointStore>")
            .field("project_store", &"<dyn ProjectStore>")
            .field("pinned_session_store", &"<dyn PinnedSessionStore>")
            .field("cron_store", &"<dyn CronStore>")
            .field("channel_store", &"<dyn ChannelStore>")
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
        Self::open_with_config_inner(data_dir, None).await
    }

    /// Open all storage backends with configuration
    ///
    /// Similar to `open`, but uses the provided configuration for store initialization.
    pub async fn open_with_config(
        data_dir: impl Into<PathBuf>,
        config: &crate::Config,
    ) -> Result<Self> {
        Self::open_with_config_inner(data_dir, Some(config)).await
    }

    /// Internal helper to open storage with optional config
    async fn open_with_config_inner(
        data_dir: impl Into<PathBuf>,
        config: Option<&crate::Config>,
    ) -> Result<Self> {
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

        // Create checkpoint directory
        let checkpoint_dir = data_dir.join("checkpoints");
        tokio::fs::create_dir_all(&checkpoint_dir)
            .await
            .map_err(|e| KernelError::storage(format!("failed to create checkpoint dir: {e}")))?;

        // Create default workspace project
        let workspace_dir = data_dir.join("workspace");
        tokio::fs::create_dir_all(&workspace_dir)
            .await
            .map_err(|e| KernelError::storage(format!("failed to create workspace dir: {e}")))?;

        // Create store instances
        let session_store: Arc<dyn super::SessionStore> =
            Arc::new(super::SqliteSessionStore::new(pool.clone()));
        let message_store: Arc<dyn super::MessageStore> =
            Arc::new(super::JsonlMessageStore::new(&sessions_dir, &data_dir));
        let usage_store: Arc<dyn super::UsageStore> =
            Arc::new(super::SqliteUsageStore::new(pool.clone()));
        let todo_store: Arc<dyn super::TodoStore> = Arc::new(super::JsonTodoStore::new(&data_dir));
        let goal_store: Arc<dyn crate::goal::GoalStore> =
            Arc::new(crate::goal::JsonGoalStore::new(&data_dir));
        let project_store: Arc<dyn super::ProjectStore> =
            Arc::new(super::SqliteProjectStore::new(pool.clone()));
        let pinned_session_store: Arc<dyn super::PinnedSessionStore> =
            Arc::new(super::SqlitePinnedSessionStore::new(pool.clone()));
        let cron_store: Arc<dyn crate::cron::CronStore> =
            Arc::new(SqliteCronStore::new(pool.clone()));
        let channel_store: Arc<dyn crate::channels::ChannelStore> = Arc::new(
            crate::channels::store::SqliteChannelStore::new(pool.clone()),
        );

        // Ensure default workspace project exists
        let default_project_id = crate::types::ProjectId::default_workspace();
        let workspace_dir_str = workspace_dir.to_str().unwrap_or("");
        if project_store.get(&default_project_id).await?.is_none() {
            project_store
                .create(&default_project_id, "Default", workspace_dir_str)
                .await?;
        }

        // Create checkpoint store with optional config
        let checkpoint_store: Arc<dyn crate::checkpoint::CheckpointStore> =
            if let Some(cfg) = config {
                Arc::new(
                    crate::checkpoint::FilesystemCheckpointStore::with_max_checkpoints(
                        &data_dir,
                        cfg.max_checkpoints,
                    ),
                )
            } else {
                Arc::new(crate::checkpoint::FilesystemCheckpointStore::new(&data_dir))
            };

        Ok(Self {
            pool,
            data_dir,
            session_store,
            message_store,
            usage_store,
            goal_store,
            todo_store,
            checkpoint_store,
            project_store,
            pinned_session_store,
            cron_store,
            channel_store,
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
                .pragma("journal_mode", "WAL")
                .pragma("foreign_keys", "ON");

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

    /// Get the goal store
    pub fn goal_store(&self) -> Arc<dyn crate::goal::GoalStore> {
        self.goal_store.clone()
    }

    /// Get the todo store
    pub fn todo_store(&self) -> Arc<dyn super::TodoStore> {
        self.todo_store.clone()
    }

    /// Get the checkpoint store
    pub fn checkpoint_store(&self) -> Arc<dyn crate::checkpoint::CheckpointStore> {
        self.checkpoint_store.clone()
    }

    /// Get the project store
    pub fn project_store(&self) -> Arc<dyn super::ProjectStore> {
        self.project_store.clone()
    }

    /// Get the data directory path
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Get the pinned session store
    pub fn pinned_session_store(&self) -> Arc<dyn super::PinnedSessionStore> {
        self.pinned_session_store.clone()
    }

    /// Get the cron store
    pub fn cron_store(&self) -> Arc<dyn crate::cron::CronStore> {
        self.cron_store.clone()
    }

    /// Get the channel store
    pub fn channel_store(&self) -> Arc<dyn crate::channels::ChannelStore> {
        self.channel_store.clone()
    }
    ///
    /// File state stores are per-session, so this returns a new instance each time
    pub fn file_state_store(&self, session_id: &str) -> super::JsonlFileStateStore {
        super::JsonlFileStateStore::new(session_id, &self.data_dir)
    }
}

#[cfg(test)]
#[path = "init_test.rs"]
mod tests;
