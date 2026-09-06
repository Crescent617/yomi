use super::*;

use crate::cron::{CronAction, CronJobId, SqliteCronStore, NEVER_EXPIRES, UNLIMITED_MAX_RUNS};
use crate::storage::migrations::run_migrations;
use chrono::Utc;
use sqlx::sqlite::SqlitePoolOptions;

fn make_job(name: &str, session_id: Option<&str>, status: CronJobStatus) -> CronJob {
    CronJob {
        id: CronJobId::new(),
        name: name.to_string(),
        schedule: "0 9 * * 1-5".to_string(),
        action: CronAction::SendMessage {
            session_id: session_id.map(str::to_string),
            content: "日报 {{date}}".to_string(),
            session_template: None,
        },
        status,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        next_run_at: Some(Utc::now()),
        last_run_at: None,
        run_count: 0,
        max_runs: UNLIMITED_MAX_RUNS,
        expires_at: NEVER_EXPIRES,
        last_error: None,
        precheck: None,
    }
}

async fn store_with(jobs: &[CronJob]) -> Arc<dyn CronStore> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let store = SqliteCronStore::new(pool);
    for job in jobs {
        store.create(job).await.unwrap();
    }
    Arc::new(store)
}

#[tokio::test]
async fn read_jobs_lists_all_active_first() {
    let target = make_job("目标", Some("sess_a"), CronJobStatus::Completed);
    let active = make_job("活跃", Some("sess_t1"), CronJobStatus::Active);
    let other = make_job("别人", Some("sess_b"), CronJobStatus::Active);
    let store = store_with(&[target.clone(), active.clone(), other]).await;

    // 全量列出（不筛归属）：completed 不显示（hrli 2026-08-22），
    // active 在前（组内时间戳秒级并列，不做次序断言）。
    let jobs = read_jobs(&store).await.unwrap();
    assert_eq!(jobs.len(), 2);
    assert!(jobs
        .iter()
        .all(|j| matches!(j.status, CronJobStatus::Active)));
}

#[test]
fn card_renders_rows_actions_and_empty_state() {
    let job = make_job("日报", Some("sess_a"), CronJobStatus::Active);
    let card = cron_card("chat-1", &[job], None);
    assert!(card.contains("⏰ Cron jobs · all chats"), "{card}");
    assert!(card.contains("日报"), "{card}");
    assert!(card.contains("0 9 * * 1-5"), "{card}");
    assert!(!card.contains("cron_trigger"), "no trigger button: {card}");
    assert!(card.contains("cron_pause"), "{card}");
    assert!(card.contains("cron_del_ask"), "{card}");
    assert!(card.contains("cron_refresh"), "{card}");

    let empty = cron_card("chat-1", &[], None);
    assert!(empty.contains("暂无定时任务"), "{empty}");

    let confirming = cron_card("chat-1", &[], Some(("cron_x", "日报")));
    assert!(confirming.contains("确认删除「日报」"), "{confirming}");
    assert!(confirming.contains("cron_del_do"), "{confirming}");

    let paused = make_job("暂停", Some("sess_a"), CronJobStatus::Paused);
    let paused_card = cron_card("chat-1", &[paused], None);
    assert!(paused_card.contains("cron_resume"), "{paused_card}");
    assert!(!paused_card.contains("cron_pause"), "{paused_card}");
}

// ── apply_action 变更臂（整卷复审 should-fix #2 的回归网）────────────

#[tokio::test]
async fn apply_action_pause_resume_flip_status_and_mark_dirty() {
    let job = make_job("日报", Some("sess_a"), CronJobStatus::Active);
    let store = store_with(std::slice::from_ref(&job)).await;

    let outcome = apply_action(
        &store,
        &serde_json::json!({ "action": "cron_pause", "id": job.id.0 }),
    )
    .await
    .unwrap();
    assert!(outcome.scheduler_dirty && outcome.confirming.is_none());
    let got = store.get(&job.id).await.unwrap().unwrap();
    assert!(matches!(got.status, CronJobStatus::Paused));

    let outcome = apply_action(
        &store,
        &serde_json::json!({ "action": "cron_resume", "id": job.id.0 }),
    )
    .await
    .unwrap();
    assert!(outcome.scheduler_dirty && outcome.confirming.is_none());
    let got = store.get(&job.id).await.unwrap().unwrap();
    assert!(matches!(got.status, CronJobStatus::Active));
}

#[tokio::test]
async fn apply_action_delete_two_phase() {
    let job = make_job("日报", Some("sess_a"), CronJobStatus::Active);
    let store = store_with(std::slice::from_ref(&job)).await;

    // 第一段：只出确认态，不动数据、不标脏。
    let outcome = apply_action(
        &store,
        &serde_json::json!({ "action": "cron_del_ask", "id": job.id.0, "name": "日报" }),
    )
    .await
    .unwrap();
    assert!(!outcome.scheduler_dirty);
    assert_eq!(
        outcome.confirming,
        Some((job.id.0.to_string(), "日报".to_string()))
    );
    assert!(store.get(&job.id).await.unwrap().is_some());

    // 第二段：删除并标脏。
    let outcome = apply_action(
        &store,
        &serde_json::json!({ "action": "cron_del_do", "id": job.id.0 }),
    )
    .await
    .unwrap();
    assert!(outcome.scheduler_dirty && outcome.confirming.is_none());
    assert!(store.get(&job.id).await.unwrap().is_none());
}

#[tokio::test]
async fn apply_action_refresh_and_unknown_are_noop() {
    let job = make_job("日报", Some("sess_a"), CronJobStatus::Active);
    let store = store_with(std::slice::from_ref(&job)).await;

    let outcome = apply_action(&store, &serde_json::json!({ "action": "cron_refresh" }))
        .await
        .unwrap();
    assert!(!outcome.scheduler_dirty && outcome.confirming.is_none());

    let outcome = apply_action(
        &store,
        &serde_json::json!({ "action": "cron_explode", "id": job.id.0 }),
    )
    .await
    .unwrap();
    assert!(!outcome.scheduler_dirty && outcome.confirming.is_none());
    let got = store.get(&job.id).await.unwrap().unwrap();
    assert!(matches!(got.status, CronJobStatus::Active));
}
