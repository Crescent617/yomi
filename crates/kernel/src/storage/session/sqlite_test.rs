use super::*;

use crate::storage::migrations::run_migrations;
use crate::storage::session::SessionStore;

async fn create_test_store() -> SqliteSessionStore {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    SqliteSessionStore::new(pool)
}

#[tokio::test]
async fn test_create_and_get() {
    let store = create_test_store().await;

    let id = SessionId::new();
    store
        .create(&id, None, None, None, None, None)
        .await
        .unwrap();
    let info = store.get(&id).await.unwrap().unwrap();

    assert_eq!(info.id.0, id.0);
    assert_eq!(info.message_count, 0);
}

#[tokio::test]
async fn test_create_with_working_dir() {
    let store = create_test_store().await;

    let id = SessionId::new();
    store
        .create(&id, None, Some("/test/dir"), None, None, None)
        .await
        .unwrap();
    let info = store.get(&id).await.unwrap().unwrap();

    assert_eq!(info.working_dir, Some("/test/dir".to_string()));
}

#[tokio::test]
async fn test_fork() {
    let store = create_test_store().await;

    let parent = SessionId::new();
    store
        .create(&parent, None, Some("/parent/dir"), None, None, None)
        .await
        .unwrap();
    let child = store.fork(&parent).await.unwrap();

    let child_info = store.get(&child).await.unwrap().unwrap();
    assert_eq!(child_info.parent_id.unwrap().0, parent.0);
    assert_eq!(child_info.working_dir, Some("/parent/dir".to_string()));
}

#[tokio::test]
async fn test_create_with_parent_id() {
    let store = create_test_store().await;
    let parent = SessionId::new();
    let child = SessionId::new();
    store
        .create(&parent, None, None, None, None, None)
        .await
        .unwrap();
    store
        .create(&child, None, None, None, Some(&parent), None)
        .await
        .unwrap();

    let info = store.get(&child).await.unwrap().unwrap();
    assert_eq!(info.parent_id.unwrap().0, parent.0);
}

#[tokio::test]
async fn test_list_ordering() {
    let store = create_test_store().await;

    let id1 = SessionId::new();
    store
        .create(&id1, None, None, None, None, None)
        .await
        .unwrap();
    let id2 = SessionId::new();
    store
        .create(&id2, None, None, None, None, None)
        .await
        .unwrap();

    // Update id1 to make it more recent
    store.update_message_count(&id1, 1).await.unwrap();

    let (list, _) = store.list(None, None, 100).await.unwrap();
    assert_eq!(list[0].id.0, id1.0);
    assert_eq!(list[1].id.0, id2.0);
}

#[tokio::test]
async fn test_list_filter_by_project_id() {
    let store = create_test_store().await;

    let pid = crate::types::ProjectId::new();
    let id1 = SessionId::new();
    store
        .create(&id1, Some(&pid), Some("/foo/bar"), None, None, None)
        .await
        .unwrap();
    let id2 = SessionId::new();
    store
        .create(&id2, None, Some("/baz/qux"), None, None, None)
        .await
        .unwrap();
    let id3 = SessionId::new();
    store
        .create(&id3, Some(&pid), Some("/foo/bar"), None, None, None)
        .await
        .unwrap();

    let (list, _) = store.list(Some(&pid), None, 100).await.unwrap();
    assert_eq!(list.len(), 2);
    let ids: Vec<_> = list.iter().map(|s| &s.id.0).collect();
    assert!(ids.contains(&&id1.0));
    assert!(ids.contains(&&id3.0));
}

#[tokio::test]
async fn test_list_limit_and_next_cursor() {
    let store = create_test_store().await;

    // Create 5 sessions with distinct updated_at timestamps to ensure pagination works
    let mut ids = Vec::new();
    for i in 0..5 {
        let id = SessionId::new();
        store
            .create(&id, None, None, None, None, None)
            .await
            .unwrap();
        ids.push(id.clone());
        store
            .update_message_count(&ids[i], i as i64 + 1)
            .await
            .unwrap();
        // Manually stagger updated_at so each session has a distinct timestamp
        let offset = format!("-{i} seconds");
        sqlx::query("UPDATE sessions SET updated_at = datetime('now', ?) WHERE id = ?")
            .bind(&offset)
            .bind(&*ids[i].0)
            .execute(&store.pool)
            .await
            .unwrap();
    }

    // Test limit
    let (list, cursor) = store.list(None, None, 2).await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(cursor.is_some());

    // Get next page using cursor
    let before = list.last().unwrap().updated_at;
    let (next_list, next_cursor) = store.list(None, Some(before), 2).await.unwrap();
    assert_eq!(next_list.len(), 2);
    assert!(next_cursor.is_some());

    // Full list for comparison
    let (full_list, full_cursor) = store.list(None, None, 100).await.unwrap();
    assert_eq!(full_list.len(), 5);
    assert!(full_cursor.is_none());

    // Next page results should be different from first page
    assert_ne!(next_list[0].id.0, list[0].id.0);
}

#[tokio::test]
async fn test_list_expired_and_delete_batch() {
    let store = create_test_store().await;

    // Create a session and manually set its updated_at to 10 days ago
    let old_id = SessionId::new();
    store
        .create(&old_id, None, Some("/test"), None, None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-10 days') WHERE id = ?")
        .bind(&*old_id.0)
        .execute(&store.pool)
        .await
        .unwrap();

    // Create a recent session
    let recent_id = SessionId::new();
    store
        .create(&recent_id, None, Some("/test"), None, None, None)
        .await
        .unwrap();

    // Expired: sessions older than 7 days
    let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
    let expired = store.list_expired(cutoff, true).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].0, old_id.0);

    let deleted = store.delete_batch(&expired).await.unwrap();
    assert_eq!(deleted, 1);

    // Verify old session is gone
    let old_session = store.get(&old_id).await.unwrap();
    assert!(old_session.is_none());

    // Verify recent session still exists
    let recent_session = store.get(&recent_id).await.unwrap();
    assert!(recent_session.is_some());
}

#[tokio::test]
async fn test_list_expired_empty_when_no_old_sessions() {
    let store = create_test_store().await;

    // Create only recent sessions
    let id1 = SessionId::new();
    store
        .create(&id1, None, None, None, None, None)
        .await
        .unwrap();
    let id2 = SessionId::new();
    store
        .create(&id2, None, None, None, None, None)
        .await
        .unwrap();

    let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
    let expired = store.list_expired(cutoff, true).await.unwrap();
    assert!(expired.is_empty());

    // Verify all sessions still exist
    let (all, _) = store.list(None, None, 100).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_list_expired_cascades_to_subagent_sessions() {
    let store = create_test_store().await;

    // Create a parent session with an old updated_at
    let parent_id = SessionId::new();
    store
        .create(&parent_id, None, Some("/test"), None, None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-10 days') WHERE id = ?")
        .bind(&*parent_id.0)
        .execute(&store.pool)
        .await
        .unwrap();

    // Create a child subagent session (recent - should still cascade with parent)
    let child_id = SessionId::new_subagent();
    store
        .create(&child_id, None, None, None, Some(&parent_id), None)
        .await
        .unwrap();

    // Create a recent sibling session
    let recent_id = SessionId::new();
    store
        .create(&recent_id, None, Some("/test"), None, None, None)
        .await
        .unwrap();

    let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
    let expired = store.list_expired(cutoff, true).await.unwrap();
    assert_eq!(expired.len(), 2);
    let expired_ids: Vec<String> = expired.iter().map(|s| s.0.to_string()).collect();
    assert!(expired_ids.contains(&parent_id.0.to_string()));
    assert!(expired_ids.contains(&child_id.0.to_string()));

    let deleted = store.delete_batch(&expired).await.unwrap();
    assert_eq!(deleted, 2);

    // Verify both are gone
    assert!(store.get(&parent_id).await.unwrap().is_none());
    assert!(store.get(&child_id).await.unwrap().is_none());

    // Verify recent session still exists
    assert!(store.get(&recent_id).await.unwrap().is_some());
}

#[tokio::test]
async fn test_list_expired_includes_orphan_subagents() {
    let store = create_test_store().await;

    // Orphan subagent: no parent, old
    let orphan_id = SessionId::new_subagent();
    store
        .create(&orphan_id, None, None, None, None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-10 days') WHERE id = ?")
        .bind(&*orphan_id.0)
        .execute(&store.pool)
        .await
        .unwrap();

    // Subagent with a *live* parent: must NOT be collected even if old
    let live_parent = SessionId::new();
    store
        .create(&live_parent, None, None, None, None, None)
        .await
        .unwrap();
    let protected_child = SessionId::new_subagent();
    store
        .create(&protected_child, None, None, None, Some(&live_parent), None)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-10 days') WHERE id = ?")
        .bind(&*protected_child.0)
        .execute(&store.pool)
        .await
        .unwrap();

    let cutoff = chrono::Utc::now() - chrono::Duration::days(7);
    let expired = store.list_expired(cutoff, true).await.unwrap();
    let expired_ids: Vec<String> = expired.iter().map(|s| s.0.to_string()).collect();
    assert!(expired_ids.contains(&orphan_id.0.to_string()));
    assert!(!expired_ids.contains(&protected_child.0.to_string()));
}

#[tokio::test]
async fn test_list_expired_respects_pinned() {
    let store = create_test_store().await;

    let pinned_id = SessionId::new();
    store
        .create(&pinned_id, None, None, None, None, None)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-10 days') WHERE id = ?")
        .bind(&*pinned_id.0)
        .execute(&store.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO pinned_sessions (session_id) VALUES (?)")
        .bind(&*pinned_id.0)
        .execute(&store.pool)
        .await
        .unwrap();

    let cutoff = chrono::Utc::now() - chrono::Duration::days(7);

    // keep_pinned = true: pinned session survives
    let expired = store.list_expired(cutoff, true).await.unwrap();
    assert!(expired.is_empty());

    // keep_pinned = false: pinned session is collected
    let expired = store.list_expired(cutoff, false).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].0, pinned_id.0);
}

#[tokio::test]
async fn test_list_excludes_subagent_sessions() {
    let store = create_test_store().await;

    let parent_id = SessionId::new();
    store
        .create(&parent_id, None, None, None, None, None)
        .await
        .unwrap();

    let child_id = SessionId::new_subagent();
    store
        .create(&child_id, None, None, None, Some(&parent_id), None)
        .await
        .unwrap();

    let (list, _) = store.list(None, None, 100).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id.0, parent_id.0);
}
