//! Database migration system for `SQLite` storage
//!
//! Tracks schema version in `_schema_migrations` table and applies
//! pending migrations in order.

use crate::types::{KernelError, Result};
use sqlx::sqlite::SqlitePool;
use tracing::{info, warn};

/// Current schema version - bump this when adding new migrations
pub const CURRENT_SCHEMA_VERSION: i64 = 17;

/// A single database migration (can contain multiple SQL statements)
struct Migration {
    version: i64,
    name: &'static str,
    sqls: &'static [&'static str],
}

/// List of all migrations in order
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 0,
        name: "initial_schema",
        sqls: &[
            r"CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                parent_id TEXT,
                title TEXT,
                message_count INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (parent_id) REFERENCES sessions(id) ON DELETE SET NULL
            );",
            r"CREATE INDEX IF NOT EXISTS idx_sessions_updated_at
               ON sessions(updated_at DESC);",
        ],
    },
    Migration {
        version: 1,
        name: "add_working_dir",
        sqls: &[r"ALTER TABLE sessions ADD COLUMN working_dir TEXT;"],
    },
    Migration {
        version: 3,
        name: "add_token_usage",
        sqls: &[
            r"CREATE TABLE token_usage (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                prompt_tokens INTEGER NOT NULL,
                completion_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                cached_tokens INTEGER,
                model TEXT,
                provider TEXT,
                usage_type TEXT NOT NULL CHECK(usage_type IN ('normal', 'subagent', 'compactor')),
                created_at TEXT NOT NULL
            );",
            r"CREATE INDEX idx_token_session ON token_usage(session_id);",
            r"CREATE INDEX idx_token_type ON token_usage(session_id, usage_type);",
        ],
    },
    Migration {
        version: 4,
        name: "add_projects",
        sqls: &[
            r"CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                dir TEXT NOT NULL UNIQUE,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
            );",
            r"CREATE UNIQUE INDEX idx_projects_dir ON projects(dir);",
            r"ALTER TABLE sessions ADD COLUMN project_id TEXT;",
            r"CREATE INDEX idx_sessions_project_id ON sessions(project_id);",
        ],
    },
    Migration {
        version: 5,
        name: "add_auto_approve_level",
        sqls: &[r"ALTER TABLE sessions ADD COLUMN auto_approve_level TEXT;"],
    },
    Migration {
        version: 6,
        name: "add_cron_jobs",
        sqls: &[
            r"CREATE TABLE cron_jobs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                schedule TEXT NOT NULL,
                action TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('active', 'paused', 'completed', 'failed')),
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                next_run_at TEXT,
                last_run_at TEXT,
                run_count INTEGER NOT NULL DEFAULT 0,
                max_runs INTEGER,
                expires_at TEXT,
                last_error TEXT
            );",
            r"CREATE INDEX idx_cron_jobs_status_next_run ON cron_jobs(status, next_run_at);",
            r"CREATE INDEX idx_cron_jobs_next_run_active ON cron_jobs(next_run_at) WHERE status = 'active';",
        ],
    },
    Migration {
        version: 7,
        name: "add_session_pinned_emoji",
        sqls: &[
            r"ALTER TABLE sessions ADD COLUMN is_pinned INTEGER NOT NULL DEFAULT 0;",
            r"ALTER TABLE sessions ADD COLUMN icon_emoji TEXT;",
        ],
    },
    Migration {
        version: 8,
        name: "pinned_sessions_table",
        sqls: &[
            r"CREATE TABLE pinned_sessions (
                session_id TEXT PRIMARY KEY,
                icon_emoji TEXT,
                pinned_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );",
            r"CREATE INDEX idx_pinned_sessions_pinned_at ON pinned_sessions(pinned_at DESC);",
            r"ALTER TABLE sessions DROP COLUMN is_pinned;",
            r"ALTER TABLE sessions DROP COLUMN icon_emoji;",
        ],
    },
    Migration {
        version: 9,
        name: "add_channel_session_mappings",
        sqls: &[
            r"CREATE TABLE channel_session_mappings (
                channel_name TEXT NOT NULL,
                external_chat_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (channel_name, external_chat_id)
            );",
            r"CREATE INDEX idx_channel_mapping_session ON channel_session_mappings(session_id);",
        ],
    },
    Migration {
        version: 10,
        name: "add_channel_routing_columns",
        sqls: &[
            r"ALTER TABLE channel_session_mappings ADD COLUMN actual_chat_id TEXT;",
            r"ALTER TABLE channel_session_mappings ADD COLUMN reply_msg_id TEXT;",
            r"UPDATE channel_session_mappings SET actual_chat_id = external_chat_id WHERE actual_chat_id IS NULL;",
        ],
    },
    Migration {
        version: 11,
        name: "remove_agent_id_from_token_usage",
        sqls: &[r"ALTER TABLE token_usage DROP COLUMN agent_id;"],
    },
    Migration {
        version: 12,
        name: "add_agent_id_to_token_usage",
        sqls: &[r"ALTER TABLE token_usage ADD COLUMN agent_id TEXT;"],
    },
    Migration {
        version: 13,
        name: "add_session_model_key",
        sqls: &[r"ALTER TABLE sessions ADD COLUMN model_key TEXT;"],
    },
    Migration {
        version: 14,
        name: "add_favorite_answers",
        sqls: &[
            r"CREATE TABLE favorite_answers (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                session_title TEXT,
                content TEXT NOT NULL,
                note TEXT,
                favorited_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                message_created_at DATETIME
            );",
            r"CREATE UNIQUE INDEX idx_favorite_answers_message
               ON favorite_answers(session_id, message_id);",
            r"CREATE INDEX idx_favorite_answers_time
               ON favorite_answers(favorited_at DESC);",
        ],
    },
    Migration {
        version: 15,
        name: "add_channel_history_cursors",
        sqls: &[r"CREATE TABLE channel_history_cursors (
                channel_name TEXT NOT NULL,
                container_id TEXT NOT NULL,
                cursor_ts INTEGER NOT NULL,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (channel_name, container_id)
            );"],
    },
    Migration {
        version: 16,
        name: "add_channel_doc_permission_requests",
        sqls: &[
            r"CREATE TABLE channel_doc_permission_requests (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_name TEXT NOT NULL,
                file_token TEXT NOT NULL,
                file_type TEXT NOT NULL,
                permission TEXT NOT NULL,
                remark TEXT,
                applicant_users TEXT NOT NULL DEFAULT '[]',
                applicant_chats TEXT NOT NULL DEFAULT '[]',
                applicant_departments TEXT NOT NULL DEFAULT '[]',
                status TEXT NOT NULL DEFAULT 'pending',
                notify_msg_ids TEXT NOT NULL DEFAULT '[]',
                resolved_by TEXT,
                resolved_perm TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                resolved_at DATETIME
            );",
            r"CREATE INDEX idx_channel_perm_requests_status
               ON channel_doc_permission_requests(channel_name, status);",
        ],
    },
    Migration {
        version: 17,
        name: "add_channel_run_subscriptions",
        sqls: &[
            r"CREATE TABLE channel_run_subscriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_name TEXT NOT NULL,
                scope_key TEXT NOT NULL,
                chat_id TEXT NOT NULL,
                recursive INTEGER NOT NULL DEFAULT 0,
                subscriber_open_id TEXT NOT NULL,
                target_chat_id TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(channel_name, scope_key, subscriber_open_id)
            );",
            r"CREATE INDEX idx_channel_run_subs_match
               ON channel_run_subscriptions(channel_name, chat_id);",
        ],
    },
];

/// Initialize migrations table and run pending migrations
pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    // For SQLite memory mode, we need to ensure all operations use the same connection.
    // We use pool.begin() to get a transaction which manages its own connection.
    // Begin transaction for all migration operations
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| KernelError::storage(format!("Failed to begin migration transaction: {e}")))?;

    // Ensure migrations table exists (this is idempotent, safe inside transaction)
    sqlx::query(
        r"CREATE TABLE IF NOT EXISTS _schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| KernelError::storage(format!("Failed to create _schema_migrations table: {e}")))?;

    // Get current version
    let current_version: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), -1) FROM _schema_migrations")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| KernelError::storage(format!("Failed to query schema version: {e}")))?;

    info!("Current database schema version: {}", current_version);

    // Find and apply pending migrations
    for migration in MIGRATIONS {
        if migration.version > current_version {
            info!(
                "Applying migration {}: {}...",
                migration.version, migration.name
            );

            // Execute each SQL statement in the migration
            for sql in migration.sqls {
                sqlx::query(*sql).execute(&mut *tx).await.map_err(|e| {
                    KernelError::storage(format!(
                        "Failed to apply migration {} ({}): SQL: {}: {e}",
                        migration.version,
                        migration.name,
                        sql.trim()
                    ))
                })?;
            }

            // Update schema version
            sqlx::query(
                "INSERT OR REPLACE INTO _schema_migrations (version, name, applied_at) VALUES (?, ?, CURRENT_TIMESTAMP)",
            )
            .bind(migration.version)
            .bind(migration.name)
            .execute(&mut *tx)
            .await
            .map_err(|e| KernelError::storage(format!("Failed to update schema version: {e}")))?;

            info!("Migration {} applied successfully", migration.version);
        }
    }

    // Verify final version matches expected
    let final_version: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), -1) FROM _schema_migrations")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| {
                KernelError::storage(format!("Failed to query final schema version: {e}"))
            })?;

    if final_version == CURRENT_SCHEMA_VERSION {
        info!("Database schema is up to date (version {})", final_version);
    } else {
        warn!(
            "Database schema version ({}) does not match expected version ({}). \
             Some migrations may have been skipped.",
            final_version, CURRENT_SCHEMA_VERSION
        );
    }

    // Commit all migration operations
    tx.commit().await.map_err(|e| {
        KernelError::storage(format!("Failed to commit migration transaction: {e}"))
    })?;

    Ok(())
}

#[cfg(test)]
#[path = "migrations_test.rs"]
mod tests;
