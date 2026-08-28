use super::*;
use crate::storage::{NewSession, StorageSet};
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
        .create(NewSession {
            working_dir: Some("/test".into()),
            ..NewSession::new(id.clone())
        })
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
    // RULE.md
    let rules_dir = sessions_dir.join("rules");
    tokio::fs::create_dir_all(&rules_dir).await.unwrap();
    tokio::fs::write(rules_dir.join(format!("{}.md", id.0)), b"rule")
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
        .save_mapping(
            "telegram",
            &format!("chat-{}", id.0),
            id,
            "chat",
            None,
            crate::channels::MappingKind::Normal,
        )
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
        sessions_dir.join("rules").join(format!("{}.md", id.0)),
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
            retention_days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    // old session collected, recent untouched
    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.sessions[0].0, old_id.0);
    assert_eq!(report.files_deleted, 5);
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

/// The documented gc edge of watch: collecting a watched chat's session
/// deletes its mapping, and with it the watch state (the row's kind is
/// gone) — a long-silent chat's watch silently turns off.
#[tokio::test]
async fn test_gc_watch_mapping_deletion_ends_watch_state() {
    let (_tmp, storage) = setup().await;
    let old_id = create_full_session(&storage, true).await;
    let chat_key = format!("chat-{}", old_id.0);

    // create_full_session 已建 kind=normal 的 chat 行；flip 成 watch
    // 必须走显式 update_mapping（save_mapping 只在建行时写 kind）。
    storage
        .channel_store()
        .update_mapping(
            "telegram",
            &chat_key,
            None,
            Some(crate::channels::MappingKind::Watch),
        )
        .await
        .unwrap();
    assert!(matches!(
        storage
            .channel_store()
            .find_mapping_kind("telegram", &chat_key)
            .await
            .unwrap(),
        Some((_, crate::channels::MappingKind::Watch))
    ));

    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();
    assert!(report.channel_mappings_deleted >= 1);
    assert!(
        storage
            .channel_store()
            .find_mapping_kind("telegram", &chat_key)
            .await
            .unwrap()
            .is_none(),
        "gc of the watched chat's session silently ends the watch"
    );
}

#[tokio::test]
async fn test_gc_dry_run_deletes_nothing() {
    let (_tmp, storage) = setup().await;

    let old_id = create_full_session(&storage, true).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
            dry_run: true,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert!(report.dry_run);
    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.files_deleted, 5);
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
            retention_days: 90,
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
            retention_days: 90,
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
        .create(NewSession {
            parent_id: Some(parent_id.clone()),
            ..NewSession::new(child_id.clone())
        })
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
        .create(NewSession::new(orphan_id.clone()))
        .await
        .unwrap();
    age_session(&storage, &orphan_id).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
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
        .create(NewSession::new(live_id.clone()))
        .await
        .unwrap();
    let live_msg = sessions_dir.join(format!("{}.jsonl", live_id.0));
    tokio::fs::write(&live_msg, b"{}\n").await.unwrap();

    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
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
            retention_days: 90,
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
        .create(NewSession::new(id.clone()))
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
            retention_days: 0, // clamped to 1 internally
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert!(report.sessions.is_empty());
    assert!(storage.session_store().get(&id).await.unwrap().is_some());
}

#[tokio::test]
async fn test_purge_sessions_ignores_age_and_pin() {
    let (_tmp, storage) = setup().await;

    // A *recent* and *pinned* session: run() would never touch it, but
    // purge_sessions must delete it (caller decides what to delete).
    let id = create_full_session(&storage, false).await;
    sqlx::query("INSERT INTO pinned_sessions (session_id) VALUES (?)")
        .bind(&*id.0)
        .execute(storage.pool())
        .await
        .unwrap();

    // token_usage row must survive
    sqlx::query(
        "INSERT INTO token_usage (id, session_id, prompt_tokens, completion_tokens, total_tokens, usage_type, created_at)
         VALUES ('u1', ?, 1, 2, 3, 'normal', datetime('now'))",
    )
    .bind(&*id.0)
    .execute(storage.pool())
    .await
    .unwrap();

    let report = storage
        .gc()
        .purge_sessions(std::slice::from_ref(&id))
        .await
        .unwrap();

    assert_eq!(report.sessions.len(), 1);
    assert_eq!(report.files_deleted, 5);
    assert_eq!(report.checkpoint_dirs_deleted, 1);
    assert_eq!(report.channel_mappings_deleted, 1);
    assert!(report.errors.is_empty());

    assert!(storage.session_store().get(&id).await.unwrap().is_none());
    for p in session_paths(&storage, &id) {
        assert!(!p.exists(), "should be deleted: {}", p.display());
    }
    let usage_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM token_usage")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(usage_count, 1, "token_usage must never be deleted");
}

#[tokio::test]
async fn test_purge_sessions_empty_list_is_noop() {
    let (_tmp, storage) = setup().await;
    let id = create_full_session(&storage, false).await;

    let report = storage.gc().purge_sessions(&[]).await.unwrap();
    assert!(report.sessions.is_empty());
    assert_eq!(report.files_deleted, 0);
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
            retention_days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.checkpoint_dirs_deleted, 1);
    assert!(!cp_dir.exists());
}

async fn create_asset(storage: &StorageSet, name: &str, stale: bool) -> std::path::PathBuf {
    let assets_dir = storage.data_dir().join("assets");
    tokio::fs::create_dir_all(&assets_dir).await.unwrap();
    let path = assets_dir.join(name);
    tokio::fs::write(&path, b"asset-data").await.unwrap();
    if stale {
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_hours(2);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(old_time).unwrap();
    }
    path
}

async fn write_asset_reference(storage: &StorageSet, id: &SessionId, name: &str) {
    let path = storage
        .data_dir()
        .join("sessions")
        .join(format!("{}.jsonl", id.0));
    let message = serde_json::json!({
        "content": [{"type": "image_url", "image_url": {"url": format!("asset://{name}")}}]
    });
    tokio::fs::write(path, format!("{message}\n"))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_gc_sweeps_only_stale_unreferenced_assets() {
    let (_tmp, storage) = setup().await;
    let live_id = create_full_session(&storage, false).await;
    write_asset_reference(&storage, &live_id, "shared.png").await;

    let referenced = create_asset(&storage, "shared.png", true).await;
    let orphan = create_asset(&storage, "orphan.png", true).await;
    let fresh = create_asset(&storage, "fresh.png", false).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.assets_deleted, 1);
    assert!(referenced.exists());
    assert!(!orphan.exists());
    assert!(fresh.exists());
}

#[tokio::test]
async fn test_gc_asset_dry_run_ignores_victim_references() {
    let (_tmp, storage) = setup().await;
    let old_id = create_full_session(&storage, true).await;
    write_asset_reference(&storage, &old_id, "expired.png").await;
    let asset = create_asset(&storage, "expired.png", true).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
            dry_run: true,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.assets_deleted, 1);
    assert!(asset.exists(), "dry-run must not delete the asset");
}

#[tokio::test]
async fn test_gc_asset_sweep_skips_all_on_malformed_live_history() {
    let (_tmp, storage) = setup().await;
    let live_id = create_full_session(&storage, false).await;
    let message_path = storage
        .data_dir()
        .join("sessions")
        .join(format!("{}.jsonl", live_id.0));
    tokio::fs::write(message_path, b"not-json\n").await.unwrap();
    let orphan = create_asset(&storage, "orphan.png", true).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.assets_deleted, 0);
    assert!(orphan.exists());
    assert!(report
        .errors
        .iter()
        .any(|error| error.contains("asset sweep skipped")));
}

#[tokio::test]
async fn test_gc_no_orphans_keeps_unreferenced_assets() {
    let (_tmp, storage) = setup().await;
    let orphan = create_asset(&storage, "orphan.png", true).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
            sweep_orphans: false,
            dry_run: false,
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.assets_deleted, 0);
    assert!(orphan.exists());
}

#[test]
fn options_from_config_maps_policy_fields() {
    let config = crate::config::GcConfig {
        retention_days: 7,
        keep_pinned: false,
        sweep_orphans: false,
        vacuum: true,
        auto: true,
    };

    let opts = GcOptions::from_config(&config, true);
    assert_eq!(opts.retention_days, 7);
    assert!(!opts.keep_pinned);
    assert!(!opts.sweep_orphans);
    assert!(opts.vacuum);
    assert!(opts.dry_run);
    assert!(opts.exclude_sessions.is_empty());

    // The scheduling field (auto) is not a per-run option.
    let opts = GcOptions::from_config(&config, false);
    assert!(!opts.dry_run);
}

#[tokio::test]
async fn test_gc_exclude_sessions_skips_victims() {
    let (_tmp, storage) = setup().await;
    let victim = create_full_session(&storage, true).await;
    let excluded = create_full_session(&storage, true).await;

    let report = storage
        .gc()
        .run(&GcOptions {
            dry_run: false,
            exclude_sessions: vec![excluded.clone()],
            ..GcOptions::default()
        })
        .await
        .unwrap();

    assert_eq!(report.sessions, vec![victim.clone()]);
    assert!(storage
        .session_store()
        .get(&victim)
        .await
        .unwrap()
        .is_none());
    assert!(storage
        .session_store()
        .get(&excluded)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn gc_sweeps_expired_cache_entries() {
    let (tmp, storage) = setup().await;
    let kv = storage.kv_cache().expect("kv cache");
    kv.put("ns_a", "old", "x").await.unwrap();
    kv.put("ns_b", "old2", "y").await.unwrap();
    kv.put("ns_a", "fresh", "z").await.unwrap();

    // Backdate two rows (put always writes the current time).
    let raw = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        tmp.path().join("cache.db").display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE kv SET created_at = 0 WHERE key != 'fresh'")
        .execute(&raw)
        .await
        .unwrap();
    drop(raw);

    // Dry run counts, deletes nothing.
    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
            dry_run: true,
            ..GcOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(report.cache_entries_deleted, 2);
    assert_eq!(kv.get("ns_a", "old").await.unwrap().as_deref(), Some("x"));

    // Real run deletes exactly the stale rows; vacuum covers cache.db too.
    let report = storage
        .gc()
        .run(&GcOptions {
            retention_days: 90,
            dry_run: false,
            vacuum: true,
            ..GcOptions::default()
        })
        .await
        .unwrap();
    assert_eq!(report.cache_entries_deleted, 2);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(kv.get("ns_a", "old").await.unwrap(), None);
    assert_eq!(kv.get("ns_b", "old2").await.unwrap(), None);
    assert_eq!(kv.get("ns_a", "fresh").await.unwrap().as_deref(), Some("z"));
}
