use crate::task::types::{CreateTaskInput, Task, TaskStatus, TaskUpdates};
use anyhow::Result;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::path::Path;

pub struct SqliteTaskStorage {
    pool: Pool<Sqlite>,
}

impl SqliteTaskStorage {
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();

        // Handle special SQLite paths (in-memory databases)
        let path_str = db_path.to_string_lossy();
        let is_memory_db = path_str == ":memory:" || path_str.starts_with("file::memory:");

        let conn_str = if is_memory_db {
            // For in-memory databases, use the standard SQLite format
            format!("sqlite:{path_str}")
        } else {
            // Ensure parent directory exists for file-based databases
            if let Some(parent) = db_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            // Use absolute path
            let absolute_path = if db_path.is_absolute() {
                db_path.to_path_buf()
            } else {
                std::env::current_dir()?.join(db_path)
            };

            // sqlx requires the database file to exist before connecting
            // Create an empty file if it doesn't exist
            if !absolute_path.exists() {
                tokio::fs::File::create(&absolute_path).await?;
            }

            // Use the file:// URL format which is more reliable
            // The path needs to be URL-encoded (spaces become %20, etc.)
            let abs_path_str = absolute_path.to_string_lossy();
            #[cfg(target_os = "windows")]
            let url_path = abs_path_str.replace('\\', "/");
            #[cfg(not(target_os = "windows"))]
            let url_path = abs_path_str.to_string();

            format!("sqlite://{url_path}")
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&conn_str)
            .await?;

        // Enable foreign keys and set busy timeout (5 seconds)
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA busy_timeout = 5000")
            .execute(&pool)
            .await?;

        let storage = Self { pool };
        storage.init_schema().await?;

        Ok(storage)
    }

    async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS tasks (
                task_index INTEGER NOT NULL,
                task_list_id TEXT NOT NULL,
                subject TEXT NOT NULL,
                description TEXT NOT NULL,
                active_form TEXT,
                owner TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                metadata TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (task_list_id, task_index)
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_list ON tasks(task_list_id);
            CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(task_list_id, status);

            -- Dependency table: task depends on (is blocked by) depends_on_task
            CREATE TABLE IF NOT EXISTS task_dependencies (
                task_list_id TEXT NOT NULL,
                task_index INTEGER NOT NULL,
                depends_on_task_index INTEGER NOT NULL,
                PRIMARY KEY (task_list_id, task_index, depends_on_task_index),
                FOREIGN KEY (task_list_id, task_index) REFERENCES tasks(task_list_id, task_index) ON DELETE CASCADE,
                FOREIGN KEY (task_list_id, depends_on_task_index) REFERENCES tasks(task_list_id, task_index) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_deps_task ON task_dependencies(task_list_id, task_index);
            CREATE INDEX IF NOT EXISTS idx_deps_depends ON task_dependencies(task_list_id, depends_on_task_index);

            -- Sequence table for per-session auto-increment
            CREATE TABLE IF NOT EXISTS task_sequences (
                task_list_id TEXT PRIMARY KEY,
                next_index INTEGER NOT NULL DEFAULT 1
            );
            ",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Begin a transaction
    async fn begin_tx(&self) -> Result<sqlx::Transaction<'_, Sqlite>> {
        Ok(self.pool.begin().await?)
    }

    pub async fn create_task(&self, task_list_id: &str, input: CreateTaskInput) -> Result<Task> {
        let now = chrono::Utc::now().to_rfc3339();
        let metadata_json = input
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;

        let mut tx = self.begin_tx().await?;

        // Get next index for this task_list_id within the same transaction
        let task_index: i64 = sqlx::query_scalar(
            r"
            INSERT INTO task_sequences (task_list_id, next_index)
            VALUES (?1, 2)
            ON CONFLICT(task_list_id) DO UPDATE SET
                next_index = task_sequences.next_index + 1
            RETURNING next_index - 1
            ",
        )
        .bind(task_list_id)
        .fetch_one(&mut *tx)
        .await?;

        // Insert the task
        sqlx::query(
            r"
            INSERT INTO tasks (task_index, task_list_id, subject, description, active_form, owner, status, metadata, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'pending', ?6, ?7, ?7)
            ",
        )
        .bind(task_index)
        .bind(task_list_id)
        .bind(&input.subject)
        .bind(&input.description)
        .bind(&input.active_form)
        .bind(metadata_json)
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(Task {
            id: task_index.to_string(),
            subject: input.subject,
            description: input.description,
            active_form: input.active_form,
            owner: None,
            status: TaskStatus::Pending,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: input.metadata,
            created_at: chrono::DateTime::parse_from_rfc3339(&now)?.into(),
            updated_at: chrono::DateTime::parse_from_rfc3339(&now)?.into(),
        })
    }

    pub async fn get_task(&self, task_list_id: &str, task_index_str: &str) -> Result<Option<Task>> {
        let task_index: i64 = task_index_str.parse()?;

        let row = sqlx::query(
            r"
            SELECT task_index, subject, description, active_form, owner, status, metadata, created_at, updated_at
            FROM tasks
            WHERE task_index = ?1 AND task_list_id = ?2
            ",
        )
        .bind(task_index)
        .bind(task_list_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        // Get dependencies
        let blocks = self.get_blocks(task_list_id, task_index).await?;
        let blocked_by = self.get_blocked_by(task_list_id, task_index).await?;

        Ok(Some(self.row_to_task(&row, blocks, blocked_by)?))
    }

    async fn get_blocks(&self, task_list_id: &str, task_index: i64) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, i64>(
            // This task blocks these tasks (these tasks depend on this task)
            // So: select task_index where depends_on_task_index=this task
            r"
            SELECT task_index
            FROM task_dependencies
            WHERE task_list_id = ?1 AND depends_on_task_index = ?2
            ",
        )
        .bind(task_list_id)
        .bind(task_index)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|idx| idx.to_string()).collect())
    }

    async fn get_blocked_by(&self, task_list_id: &str, task_index: i64) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, i64>(
            // This task is blocked by (depends on) these tasks
            r"
            SELECT depends_on_task_index
            FROM task_dependencies
            WHERE task_list_id = ?1 AND task_index = ?2
            ",
        )
        .bind(task_list_id)
        .bind(task_index)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|idx| idx.to_string()).collect())
    }

    #[allow(clippy::unused_self)]
    fn row_to_task(
        &self,
        row: &sqlx::sqlite::SqliteRow,
        blocks: Vec<String>,
        blocked_by: Vec<String>,
    ) -> Result<Task> {
        let status_str: String = row.try_get("status")?;
        let status = match status_str.as_str() {
            "in_progress" => TaskStatus::InProgress,
            "completed" => TaskStatus::Completed,
            _ => TaskStatus::Pending,
        };

        let metadata: Option<String> = row.try_get("metadata")?;
        let metadata = metadata.map(|m| serde_json::from_str(&m)).transpose()?;

        Ok(Task {
            id: row.try_get::<i64, _>("task_index")?.to_string(),
            subject: row.try_get("subject")?,
            description: row.try_get("description")?,
            active_form: row.try_get("active_form")?,
            owner: row.try_get("owner")?,
            status,
            blocks,
            blocked_by,
            metadata,
            created_at: chrono::DateTime::parse_from_rfc3339(
                &row.try_get::<String, _>("created_at")?,
            )?
            .into(),
            updated_at: chrono::DateTime::parse_from_rfc3339(
                &row.try_get::<String, _>("updated_at")?,
            )?
            .into(),
        })
    }

    pub async fn update_task(
        &self,
        task_list_id: &str,
        task_index_str: &str,
        updates: TaskUpdates,
    ) -> Result<Option<Task>> {
        let task_index: i64 = task_index_str.parse()?;
        let now = chrono::Utc::now().to_rfc3339();

        let mut tx = self.begin_tx().await?;

        // Check if task exists
        let exists: bool = sqlx::query_scalar(
            r"SELECT EXISTS(SELECT 1 FROM tasks WHERE task_index = ?1 AND task_list_id = ?2)",
        )
        .bind(task_index)
        .bind(task_list_id)
        .fetch_one(&mut *tx)
        .await?;

        if !exists {
            tx.rollback().await?;
            return Ok(None);
        }

        // Build dynamic UPDATE - atomic single statement
        let mut set_clauses = vec!["updated_at = ?".to_string()];
        let mut params_count = 1;

        if updates.subject.is_some() {
            params_count += 1;
            set_clauses.push(format!("subject = ?{params_count}"));
        }
        if updates.description.is_some() {
            params_count += 1;
            set_clauses.push(format!("description = ?{params_count}"));
        }
        if updates.active_form.is_some() {
            params_count += 1;
            set_clauses.push(format!("active_form = ?{params_count}"));
        }
        if updates.owner.is_some() {
            params_count += 1;
            set_clauses.push(format!("owner = ?{params_count}"));
        }
        if updates.status.is_some() {
            params_count += 1;
            set_clauses.push(format!("status = ?{params_count}"));
        }
        if updates.metadata.is_some() {
            params_count += 1;
            set_clauses.push(format!("metadata = ?{params_count}"));
        }

        // Only update if there are actual changes
        if params_count > 1 {
            let sql = format!(
                "UPDATE tasks SET {} WHERE task_index = ?{} AND task_list_id = ?{}",
                set_clauses.join(", "),
                params_count + 1,
                params_count + 2
            );

            let mut query = sqlx::query(&sql).bind(&now);

            if let Some(subject) = &updates.subject {
                query = query.bind(subject);
            }
            if let Some(description) = &updates.description {
                query = query.bind(description);
            }
            if let Some(active_form) = &updates.active_form {
                query = query.bind(active_form);
            }
            if let Some(owner) = &updates.owner {
                query = query.bind(owner);
            }
            if let Some(status) = &updates.status {
                query = query.bind(status.to_string());
            }
            if let Some(metadata) = &updates.metadata {
                query = query.bind(serde_json::to_string(metadata)?);
            }

            query
                .bind(task_index)
                .bind(task_list_id)
                .execute(&mut *tx)
                .await?;
        }

        // Update dependencies within same transaction
        // Handle blocked_by: this task is blocked by (depends on) these tasks
        if let Some(blocked_by) = updates.blocked_by {
            // First delete existing dependencies where this task is the dependent
            sqlx::query(
                r"DELETE FROM task_dependencies WHERE task_list_id = ?1 AND task_index = ?2",
            )
            .bind(task_list_id)
            .bind(task_index)
            .execute(&mut *tx)
            .await?;

            // Insert new dependencies
            for blocker_idx_str in blocked_by {
                let blocker_idx: i64 = blocker_idx_str.parse()?;
                sqlx::query(
                    r"INSERT INTO task_dependencies (task_list_id, task_index, depends_on_task_index) VALUES (?1, ?2, ?3)"
                )
                .bind(task_list_id)
                .bind(task_index)
                .bind(blocker_idx)
                .execute(&mut *tx)
                .await?;
            }
        }

        // Handle blocks: this task blocks (is depended on by) these tasks
        if let Some(blocks) = updates.blocks {
            // First delete existing dependencies where this task is the blocker
            sqlx::query(
                r"DELETE FROM task_dependencies WHERE task_list_id = ?1 AND depends_on_task_index = ?2"
            )
            .bind(task_list_id)
            .bind(task_index)
            .execute(&mut *tx)
            .await?;

            // Insert new reverse dependencies (these tasks are now blocked by this task)
            for blocked_idx_str in blocks {
                let blocked_idx: i64 = blocked_idx_str.parse()?;
                sqlx::query(
                    r"INSERT INTO task_dependencies (task_list_id, task_index, depends_on_task_index) VALUES (?1, ?2, ?3)"
                )
                .bind(task_list_id)
                .bind(blocked_idx)  // The task that is blocked
                .bind(task_index)  // This task is the blocker
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;

        // Return updated task
        self.get_task(task_list_id, task_index_str).await
    }

    pub async fn delete_task(&self, task_list_id: &str, task_index_str: &str) -> Result<bool> {
        let task_index: i64 = task_index_str.parse()?;

        let result = sqlx::query(r"DELETE FROM tasks WHERE task_index = ?1 AND task_list_id = ?2")
            .bind(task_index)
            .bind(task_list_id)
            .execute(&self.pool)
            .await?;

        // Dependencies are deleted automatically via CASCADE
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_tasks(&self, task_list_id: &str) -> Result<Vec<Task>> {
        let rows = sqlx::query(
            r"
            SELECT task_index, subject, description, active_form, owner, status, metadata, created_at, updated_at
            FROM tasks
            WHERE task_list_id = ?1
            ORDER BY task_index ASC
            ",
        )
        .bind(task_list_id)
        .fetch_all(&self.pool)
        .await?;

        let mut tasks = Vec::new();
        for row in rows {
            let task_index: i64 = row.try_get("task_index")?;
            let blocks = self.get_blocks(task_list_id, task_index).await?;
            let blocked_by = self.get_blocked_by(task_list_id, task_index).await?;
            tasks.push(self.row_to_task(&row, blocks, blocked_by)?);
        }

        Ok(tasks)
    }

    pub async fn reset_tasks(&self, task_list_id: &str) -> Result<()> {
        let mut tx = self.begin_tx().await?;

        sqlx::query(r"DELETE FROM tasks WHERE task_list_id = ?1")
            .bind(task_list_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(r"DELETE FROM task_sequences WHERE task_list_id = ?1")
            .bind(task_list_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }
}
