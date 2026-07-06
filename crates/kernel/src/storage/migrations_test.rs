use super::*;

async fn get_current_version(pool: &SqlitePool) -> Result<i64> {
    // Check if table exists first (for SQLite memory mode where each query might use different connection)
    let table_exists: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_schema_migrations'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false);

    if !table_exists {
        return Ok(-1);
    }

    // Check if _schema_migrations has any entries
    let version: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM _schema_migrations")
        .fetch_optional(pool)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to query schema version: {e}")))?;

    Ok(version.unwrap_or(-1))
}

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
    assert_eq!(
        version, CURRENT_SCHEMA_VERSION,
        "Schema version should match current"
    );

    // Verify sessions table was created
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "sessions table should exist");

    // Verify working_dir column exists (from migration 1)
    let has_working_dir: bool = sqlx::query_scalar(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('sessions') WHERE name = 'working_dir'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        has_working_dir,
        "working_dir column should exist after migration 1"
    );
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
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _schema_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(
        count,
        MIGRATIONS.len() as i64,
        "All migrations should be recorded"
    );

    // Check migration names are stored
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM _schema_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();

    for (i, migration) in MIGRATIONS.iter().enumerate() {
        assert_eq!(names[i], migration.name, "Migration name should match");
    }
}
