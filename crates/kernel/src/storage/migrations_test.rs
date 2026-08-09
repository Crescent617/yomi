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

#[tokio::test]
async fn migration_19_dedupes_cron_job_names() {
    let pool = create_test_pool().await;

    // 模拟 v18 状态：迁移记录停在 18，cron_jobs 是旧 schema（name 无唯一约束）
    sqlx::query(
        r"CREATE TABLE _schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO _schema_migrations (version, name) VALUES (18, 'add_channel_mention_overrides')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // v18 存量库里还有 sessions 表（v0 建立，后续 v20 要 ALTER 它）
    sqlx::query(
        r"CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            parent_id TEXT,
            title TEXT,
            message_count INTEGER NOT NULL DEFAULT 0
        );",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
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
    )
    .execute(&pool)
    .await
    .unwrap();

    // 两个同名 job（一新一旧）+ 一个不重名的
    let action = r#"{"type":"shell","command":"true","working_dir":null}"#;
    for (id, name, updated_at) in [
        ("cron-old1", "janitor", "2026-01-01T00:00:00+00:00"),
        ("cron-new1", "janitor", "2026-02-01T00:00:00+00:00"),
        ("cron-solo", "other", "2026-01-01T00:00:00+00:00"),
    ] {
        sqlx::query(
            "INSERT INTO cron_jobs (id, name, schedule, action, status, created_at, updated_at)
             VALUES (?, ?, '0 9 * * *', ?, 'active', ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(action)
        .bind(updated_at)
        .bind(updated_at)
        .execute(&pool)
        .await
        .unwrap();
    }

    run_migrations(&pool).await.unwrap();

    // 最新的一条保留原名，旧的被改名加后缀（不删行）
    let kept: String = sqlx::query_scalar("SELECT name FROM cron_jobs WHERE id = 'cron-new1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, "janitor");
    let renamed: String = sqlx::query_scalar("SELECT name FROM cron_jobs WHERE id = 'cron-old1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(renamed, "janitor#dup-cron-old");

    // 唯一索引生效：同名插入被拒绝
    let dup = sqlx::query(
        "INSERT INTO cron_jobs (id, name, schedule, action, status, created_at, updated_at)
         VALUES ('cron-x', 'janitor', '0 9 * * *', ?, 'active',
                 '2026-03-01T00:00:00+00:00', '2026-03-01T00:00:00+00:00')",
    )
    .bind(action)
    .execute(&pool)
    .await;
    assert!(dup.is_err());

    let version = get_current_version(&pool).await.unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
}
