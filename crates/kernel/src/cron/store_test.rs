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
        precheck: None,
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
        precheck: None,
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

#[tokio::test]
async fn precheck_round_trip_and_three_state_update() {
    let store = test_store().await;
    let mut job = shell_job("sensor");
    job.precheck = Some("test -f /tmp/new".to_string());
    store.create(&job).await.unwrap();
    assert_eq!(
        store
            .get(&job.id)
            .await
            .unwrap()
            .unwrap()
            .precheck
            .as_deref(),
        Some("test -f /tmp/new")
    );

    // None = 不变
    store
        .update(
            &job.id,
            &UpdateCronJobInput {
                name: Some("sensor2".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .get(&job.id)
            .await
            .unwrap()
            .unwrap()
            .precheck
            .as_deref(),
        Some("test -f /tmp/new")
    );

    // Some(cmd) = 设置
    store
        .update(
            &job.id,
            &UpdateCronJobInput {
                precheck: Some("exit 0".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .get(&job.id)
            .await
            .unwrap()
            .unwrap()
            .precheck
            .as_deref(),
        Some("exit 0")
    );

    // Some("") = 清除（落 NULL）
    store
        .update(
            &job.id,
            &UpdateCronJobInput {
                precheck: Some(String::new()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(store.get(&job.id).await.unwrap().unwrap().precheck, None);
}

#[tokio::test]
async fn precheck_update_whitespace_only_clears() {
    let store = test_store().await;
    let mut job = shell_job("ws-gate");
    job.precheck = Some("exit 0".to_string());
    store.create(&job).await.unwrap();

    // 各种 Unicode 空白（空格/tab/换行）都归一为清除——Rust 侧归一，
    // 不依赖 SQL TRIM（它只剥 U+0020）
    for ws in ["", "\t", "  \n\t "] {
        store
            .update(
                &job.id,
                &UpdateCronJobInput {
                    precheck: Some("exit 0".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        store
            .update(
                &job.id,
                &UpdateCronJobInput {
                    precheck: Some(ws.to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store.get(&job.id).await.unwrap().unwrap().precheck,
            None,
            "whitespace {ws:?} must clear the gate"
        );
    }
}
