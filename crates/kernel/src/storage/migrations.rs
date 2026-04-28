//! Database migration system for `SQLite` storage
//!
//! Tracks schema version in `_schema_migrations` table and applies
//! pending migrations in order.

use anyhow::{Context, Result};
use sqlx::sqlite::SqlitePool;
use tracing::{info, warn};

/// Current schema version - bump this when adding new migrations
pub const CURRENT_SCHEMA_VERSION: i64 = 1;

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
];

/// Initialize migrations table and run pending migrations
pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    // For SQLite memory mode, we need to ensure all operations use the same connection.
    // We use pool.begin() to get a transaction which manages its own connection.
    // Begin transaction for all migration operations
    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin migration transaction")?;

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
    .context("Failed to create _schema_migrations table")?;

    // Get current version
    let current_version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), -1) FROM _schema_migrations",
    )
    .fetch_one(&mut *tx)
    .await
    .context("Failed to query schema version")?;

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
                sqlx::query(sql)
                    .execute(&mut *tx)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to apply migration {} ({}): SQL: {}",
                            migration.version,
                            migration.name,
                            sql.trim()
                        )
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
            .context("Failed to update schema version")?;

            info!(
                "Migration {} applied successfully",
                migration.version
            );
        }
    }

    // Verify final version matches expected
    let final_version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), -1) FROM _schema_migrations",
    )
    .fetch_one(&mut *tx)
    .await
    .context("Failed to query final schema version")?;

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
    tx.commit()
        .await
        .context("Failed to commit migration transaction")?;

    Ok(())
}

/// Get the current schema version from the database
#[cfg(test)]
async fn get_current_version(pool: &SqlitePool) -> Result<i64> {
    // Check if table exists first (for SQLite memory mode where each query might use different connection)
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_schema_migrations'"
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);
    
    if !table_exists {
        return Ok(-1);
    }
    
    // Check if _schema_migrations has any entries
    let version: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(version) FROM _schema_migrations"
    )
    .fetch_optional(pool)
    .await
    .context("Failed to query schema version")?;

    Ok(version.unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_pool() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_migrations_run_in_order() {
        let pool = create_test_pool().await;

        // Check initial version
        let initial_version = get_current_version(&pool).await.unwrap();
        assert_eq!(initial_version, -1, "Initial version should be -1");

        // Run migrations
        run_migrations(&pool).await.unwrap();

        // Check version
        let version = get_current_version(&pool).await.unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION, "Schema version should match current");

        // Verify sessions table was created
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1, "sessions table should exist");

        // Verify working_dir column exists (from migration 1)
        let has_working_dir: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('sessions') WHERE name = 'working_dir'"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(has_working_dir, "working_dir column should exist after migration 1");
    }

    #[tokio::test]
    async fn test_migrations_are_idempotent() {
        let pool = create_test_pool().await;

        // Run migrations twice
        run_migrations(&pool).await.unwrap();
        run_migrations(&pool).await.unwrap();

        // Should still be at correct version
        let version = get_current_version(&pool).await.unwrap();
        assert!(version >= 0);
    }

    #[tokio::test]
    async fn test_migrations_table_tracks_versions() {
        let pool = create_test_pool().await;

        run_migrations(&pool).await.unwrap();

        // Check that all migrations are recorded
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM _schema_migrations"
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(count, MIGRATIONS.len() as i64, "All migrations should be recorded");

        // Check migration names are stored
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM _schema_migrations ORDER BY version"
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        for (i, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(names[i], migration.name, "Migration name should match");
        }
    }
}
