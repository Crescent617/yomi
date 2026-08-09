use super::*;

fn row(max_runs: Option<i64>, expires_at: Option<&str>) -> CronJobRow {
    CronJobRow {
        id: "cron_test".to_string(),
        name: "test".to_string(),
        schedule: "0 9 * * *".to_string(),
        action: r#"{"type":"shell","command":"true","working_dir":null}"#.to_string(),
        status: "active".to_string(),
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
        next_run_at: None,
        last_run_at: None,
        run_count: 0,
        max_runs,
        expires_at: expires_at.map(str::to_string),
        last_error: None,
    }
}

#[test]
fn legacy_null_limits_read_as_sentinels() {
    let job: CronJob = row(None, None).into();
    assert_eq!(job.max_runs, UNLIMITED_MAX_RUNS);
    assert_eq!(job.expires_at, NEVER_EXPIRES);
    assert!(!job.has_max_runs());
    assert!(!job.has_expiry());
}

#[test]
fn stored_limits_round_trip() {
    let expires = "2027-01-01T00:00:00+00:00";
    let job: CronJob = row(Some(5), Some(expires)).into();
    assert_eq!(job.max_runs, 5);
    assert!(job.has_max_runs());
    assert!(job.has_expiry());
    assert_eq!(job.expires_at.to_rfc3339(), expires);
}

#[test]
fn sentinel_values_round_trip() {
    // A job written with the sentinels reads back as unlimited / never.
    let job: CronJob = row(Some(0), Some(&NEVER_EXPIRES.to_rfc3339())).into();
    assert_eq!(job.max_runs, UNLIMITED_MAX_RUNS);
    assert_eq!(job.expires_at, NEVER_EXPIRES);
}

// ── DB-backed tests (real SQLite, full migrations) ─────────────────────

async fn test_store() -> SqliteCronStore {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::storage::migrations::run_migrations(&pool)
        .await
        .unwrap();
    SqliteCronStore::new(pool)
}

fn shell_job(name: &str) -> CronJob {
    let now = Utc::now();
    CronJob {
        id: CronJobId::new(),
        name: name.to_string(),
        schedule: "0 9 * * *".to_string(),
        action: CronAction::Shell {
            command: "true".to_string(),
            working_dir: None,
        },
        status: CronJobStatus::Active,
        created_at: now,
        updated_at: now,
        next_run_at: None,
        last_run_at: None,
        run_count: 0,
        max_runs: UNLIMITED_MAX_RUNS,
        expires_at: NEVER_EXPIRES,
        last_error: None,
    }
}

#[tokio::test]
async fn get_by_name_round_trip() {
    let store = test_store().await;
    assert!(store.get_by_name("daily").await.unwrap().is_none());

    let job = shell_job("daily");
    store.create(&job).await.unwrap();

    let found = store.get_by_name("daily").await.unwrap().unwrap();
    assert_eq!(found.id.0, job.id.0);
}

#[tokio::test]
async fn duplicate_name_create_is_duplicate_name_error() {
    let store = test_store().await;
    store.create(&shell_job("daily")).await.unwrap();

    let err = store.create(&shell_job("daily")).await.unwrap_err();
    assert!(matches!(err, CronError::DuplicateName(_)));
}

#[tokio::test]
async fn duplicate_name_update_is_duplicate_name_error() {
    let store = test_store().await;
    store.create(&shell_job("a")).await.unwrap();
    let b = shell_job("b");
    store.create(&b).await.unwrap();

    let input = UpdateCronJobInput {
        name: Some("a".to_string()),
        ..Default::default()
    };
    let err = store.update(&b.id, &input).await.unwrap_err();
    assert!(matches!(err, CronError::DuplicateName(_)));
}
