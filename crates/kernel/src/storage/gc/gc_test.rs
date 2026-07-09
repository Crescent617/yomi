use super::*;
use crate::storage::StorageSet;
use crate::types::SessionId;

async fn setup() -> (tempfile::TempDir, StorageSet) {
    let tmp = tempfile::tempdir().unwrap();
    let storage = StorageSet::open(tmp.path().to_path_buf()).await.unwrap();
    (tmp, storage)
}

/// Create a session with all associated resources on disk + in db
async fn create_full_session(storage: &StorageSet, old: bool) -> SessionId {
    let id = SessionId::new();
    create_full_session_with_id(storage, &id, old).await;
    id
}

async fn create_full_session_with_id(storage: &StorageSet, id: &SessionId, old: bool) {
    storage
        .session_store()
        .create(id, None, Some("/test"), None, None, None)
        .await
        .unwrap();
    if old {
        age_session(storage, id).await;
    }

    let data_dir = storage.data_dir().to_path_buf();
    let sessions_dir = data_dir.join("sessions");

    // message history
    tokio::fs::write(sessions_dir.join(format!("{}.jsonl", id.0)), b"{}\n")
        .await
        .unwrap();
    // todo
    let todos_dir = sessions_dir.join("todos");
    tokio::fs::create_dir_all(&todos_dir).await.unwrap();
    tokio::fs::write(todos_dir.join(format!("{}.json", id.0)), b"[]")
        .await
        .unwrap();
    // goal
    let goals_dir = sessions_dir.join("goals");
    tokio::fs::create_dir_all(&goals_dir).await.unwrap();
    tokio::fs::write(goals_dir.join(format!("{}.json", id.0)), b"{}")
        .await
        .unwrap();
    // file state
    let fs_dir = sessions_dir.join("file_states");
    tokio::fs::create_dir_all(&fs_dir).await.unwrap();
    tokio::fs::write(fs_dir.join(format!("{}.jsonl", id.0)), b"{}\n")
        .await
        .unwrap();
    // checkpoint dir
    let cp_dir = data_dir.join("checkpoints").join(&*id.0);
    tokio::fs::create_dir_all(&cp_dir).await.unwrap();
    tokio::fs::write(cp_dir.join("manifest.json"), b"{}")
        .await
        .unwrap();
    // channel mapping
    storage
        .channel_store()
        .save_mapping("telegram", &format!("chat-{}", id.0), id, "chat", None)
        .await
        .unwrap();
}

async fn age_session(storage: &StorageSet, id: &SessionId) {
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-100 days') WHERE id = ?")
        .bind(&*id.0)
        .execute(storage.pool())
        .await
        .unwrap();
}

fn session_paths(storage: &StorageSet, id: &SessionId) -> Vec<std::path::PathBuf> {
    let sessions_dir = storage.data_dir().join("sessions");
    vec![
        sessions_dir.join(format!("{}.jsonl", id.0)),
        sessions_dir.join("todos").join(format!("{}.json", id.0)),
        sessions_dir.join("goals").join(format!("{}.json", id.0)),
        sessions_dir
            .join("file_states")
            .join(format!("{}.jsonl", id.0)),
        storage.data_dir().join("checkpoints").join(&*id.0),
    ]
}

#[tokio::test]
async fn test_gc_full_pipeline() {
    let (_tmp, storage) = setup().await;

    let old_id = create_full_session(&storage, true).await;
    let recent_id = create_full_session(&storage, false).await;

    // Insert a token_usage row for the old session: must survive gc
    sqlx::query(
        "INSERT INTO token_usage (id, session_id, prompt_tokens, completion_tokens, total_tokens, usage_type, created_at)
         VALUES ('u1', ?, 10, 20, 30, 'normal', datetime('now'))",
    )
    .bind(&*old_id.0)
    .execute(storage.pool())
    .await
    .unwrap();

    let report = storage
        .gc()
        .run(&GcOptions {
            days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    // old session collected, recent untouched
    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.sessions[0].0, old_id.0);
    assert_eq!(report.files_deleted, 4);
    assert_eq!(report.checkpoint_dirs_deleted, 1);
    assert_eq!(report.channel_mappings_deleted, 1);
    assert!(report.bytes_reclaimed > 0);
    assert!(report.errors.is_empty());

    // db row gone
    assert!(storage
        .session_store()
        .get(&old_id)
        .await
        .unwrap()
        .is_none());
    assert!(storage
        .session_store()
        .get(&recent_id)
        .await
        .unwrap()
        .is_some());

    // old files gone, recent files intact
    for p in session_paths(&storage, &old_id) {
        assert!(!p.exists(), "should be deleted: {}", p.display());
    }
    for p in session_paths(&storage, &recent_id) {
        assert!(p.exists(), "should survive: {}", p.display());
    }

    // token_usage rows must be untouched
    let usage_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM token_usage")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(usage_count, 1, "token_usage must never be deleted by gc");
}

#[tokio::test]
async fn test_gc_dry_run_deletes_nothing() {
    let (_tmp, storage) = setup().await;

    let old_id = create_full_session(&storage, true).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            days: 90,
            dry_run: true,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert!(report.dry_run);
    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.files_deleted, 4);
    assert_eq!(report.checkpoint_dirs_deleted, 1);
    assert!(report.bytes_reclaimed > 0);

    // Nothing actually deleted
    assert!(storage
        .session_store()
        .get(&old_id)
        .await
        .unwrap()
        .is_some());
    for p in session_paths(&storage, &old_id) {
        assert!(p.exists(), "dry-run must not delete: {}", p.display());
    }
    assert!(storage
        .channel_store()
        .find_mapping("telegram", &format!("chat-{}", old_id.0))
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn test_gc_keeps_pinned_by_default() {
    let (_tmp, storage) = setup().await;

    let pinned_id = create_full_session(&storage, true).await;
    sqlx::query("INSERT INTO pinned_sessions (session_id) VALUES (?)")
        .bind(&*pinned_id.0)
        .execute(storage.pool())
        .await
        .unwrap();

    // Default: pinned survives
    let report = storage
        .gc()
        .run(&GcOptions {
            days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();
    assert!(report.sessions.is_empty());
    assert!(storage
        .session_store()
        .get(&pinned_id)
        .await
        .unwrap()
        .is_some());

    // include pinned: collected
    let report = storage
        .gc()
        .run(&GcOptions {
            days: 90,
            keep_pinned: false,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(report.sessions.len(), 1);
    assert!(storage
        .session_store()
        .get(&pinned_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_gc_cascades_subagents() {
    let (_tmp, storage) = setup().await;

    // Old parent + recent subagent child -> both collected
    let parent_id = SessionId::new();
    create_full_session_with_id(&storage, &parent_id, true).await;

    let child_id = SessionId::new_subagent();
    storage
        .session_store()
        .create(&child_id, None, None, None, Some(&parent_id), None)
        .await
        .unwrap();
    let child_msgs = storage
        .data_dir()
        .join("sessions")
        .join(format!("{}.jsonl", child_id.0));
    tokio::fs::write(&child_msgs, b"{}\n").await.unwrap();

    // Orphan subagent (parent gone), old -> collected
    let orphan_id = SessionId::new_subagent();
    storage
        .session_store()
        .create(&orphan_id, None, None, None, None, None)
        .await
        .unwrap();
    age_session(&storage, &orphan_id).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    let ids: Vec<String> = report.sessions.iter().map(|s| s.0.to_string()).collect();
    assert!(ids.contains(&parent_id.0.to_string()));
    assert!(ids.contains(&child_id.0.to_string()));
    assert!(ids.contains(&orphan_id.0.to_string()));
    assert_eq!(report.subagent_sessions, 2);
    assert!(!child_msgs.exists());
}

#[tokio::test]
async fn test_gc_orphan_sweep() {
    let (_tmp, storage) = setup().await;

    let sessions_dir = storage.data_dir().join("sessions");
    tokio::fs::create_dir_all(sessions_dir.join("todos"))
        .await
        .unwrap();

    // Orphan message file (no db row)
    let orphan_msg = sessions_dir.join("dead-session.jsonl");
    tokio::fs::write(&orphan_msg, b"{}\n").await.unwrap();
    // Orphan todo
    let orphan_todo = sessions_dir.join("todos").join("dead-session.json");
    tokio::fs::write(&orphan_todo, b"[]").await.unwrap();
    // Orphan checkpoint dir
    let orphan_cp = storage.data_dir().join("checkpoints").join("dead-session");
    tokio::fs::create_dir_all(&orphan_cp).await.unwrap();
    tokio::fs::write(orphan_cp.join("manifest.json"), b"{}")
        .await
        .unwrap();
    // Stale .tmp (mtime in the past)
    let stale_tmp = sessions_dir.join("leftover.tmp");
    tokio::fs::write(&stale_tmp, b"x").await.unwrap();
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_hours(2);
    let f = std::fs::File::options()
        .write(true)
        .open(&stale_tmp)
        .unwrap();
    f.set_modified(old_time).unwrap();
    drop(f);
    // Fresh .tmp: must survive
    let fresh_tmp = sessions_dir.join("inflight.tmp");
    tokio::fs::write(&fresh_tmp, b"x").await.unwrap();

    // A live session's file: must survive
    let live_id = SessionId::new();
    storage
        .session_store()
        .create(&live_id, None, None, None, None, None)
        .await
        .unwrap();
    let live_msg = sessions_dir.join(format!("{}.jsonl", live_id.0));
    tokio::fs::write(&live_msg, b"{}\n").await.unwrap();

    let report = storage
        .gc()
        .run(&GcOptions {
            days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.orphan_files_deleted, 4); // msg + todo + cp dir + stale tmp
    assert!(!orphan_msg.exists());
    assert!(!orphan_todo.exists());
    assert!(!orphan_cp.exists());
    assert!(!stale_tmp.exists());
    assert!(fresh_tmp.exists(), "fresh .tmp must survive");
    assert!(live_msg.exists(), "live session file must survive");
}

#[tokio::test]
async fn test_gc_no_orphan_sweep_when_disabled() {
    let (_tmp, storage) = setup().await;

    let sessions_dir = storage.data_dir().join("sessions");
    let orphan_msg = sessions_dir.join("dead-session.jsonl");
    tokio::fs::write(&orphan_msg, b"{}\n").await.unwrap();

    let report = storage
        .gc()
        .run(&GcOptions {
            days: 90,
            sweep_orphans: false,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.orphan_files_deleted, 0);
    assert!(orphan_msg.exists());
}

#[tokio::test]
async fn test_gc_days_minimum_enforced() {
    let (_tmp, storage) = setup().await;

    // Session updated 12 hours ago must survive even with days = 0
    let id = SessionId::new();
    storage
        .session_store()
        .create(&id, None, None, None, None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-12 hours') WHERE id = ?")
        .bind(&*id.0)
        .execute(storage.pool())
        .await
        .unwrap();

    let report = storage
        .gc()
        .run(&GcOptions {
            days: 0, // clamped to 1 internally
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert!(report.sessions.is_empty());
    assert!(storage.session_store().get(&id).await.unwrap().is_some());
}

#[tokio::test]
async fn test_gc_works_with_real_checkpoint_store() {
    let (_tmp, storage) = setup().await;

    // Use the real checkpoint store to create a checkpoint, verify gc removes it
    let old_id = create_full_session(&storage, true).await;
    // remove the fake checkpoint dir created by helper; use the real store
    let cp_dir = storage.data_dir().join("checkpoints").join(&*old_id.0);
    tokio::fs::remove_dir_all(&cp_dir).await.unwrap();

    storage
        .checkpoint_store()
        .create_checkpoint(&old_id.0, "msg-1", "test checkpoint", Vec::new())
        .await
        .unwrap();
    assert!(cp_dir.exists());

    let report = storage
        .gc()
        .run(&GcOptions {
            days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.checkpoint_dirs_deleted, 1);
    assert!(!cp_dir.exists());
}
