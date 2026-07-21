use super::*;

use crate::storage::migrations::run_migrations;
use crate::types::{MessageId, SessionId};

async fn create_test_store() -> SqliteFavoriteStore {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    SqliteFavoriteStore::new(pool)
}

fn make_input(content: &str) -> AddFavoriteInput {
    AddFavoriteInput {
        session_id: SessionId::new(),
        message_id: MessageId::new(),
        session_title: Some("Test session".to_string()),
        content: content.to_string(),
        note: None,
        message_created_at: None,
    }
}

#[tokio::test]
async fn test_add_and_get_by_message() {
    let store = create_test_store().await;
    let input = make_input("hello **world**");

    let added = store.add(input.clone()).await.unwrap();
    assert!(added.id.starts_with("fav_"));
    assert_eq!(added.content, "hello **world**");
    assert_eq!(added.session_title.as_deref(), Some("Test session"));

    let fetched = store
        .get_by_message(&input.session_id, &input.message_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.id, added.id);
    assert_eq!(fetched.session_id.0, input.session_id.0);
    assert_eq!(fetched.message_id.0, input.message_id.0);
}

#[tokio::test]
async fn test_add_same_message_refreshes_snapshot() {
    let store = create_test_store().await;
    let mut input = make_input("v1");
    store.add(input.clone()).await.unwrap();

    input.content = "v2".to_string();
    input.note = Some("keep me".to_string());
    let updated = store.add(input.clone()).await.unwrap();
    assert_eq!(updated.content, "v2");
    assert_eq!(updated.note.as_deref(), Some("keep me"));

    // Existing note is preserved when re-favoriting without a note.
    input.note = None;
    let updated = store.add(input.clone()).await.unwrap();
    assert_eq!(updated.note.as_deref(), Some("keep me"));

    let all = store.list(None, 10, 0).await.unwrap();
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn test_remove() {
    let store = create_test_store().await;
    let added = store.add(make_input("bye")).await.unwrap();

    store.remove(&added.id).await.unwrap();
    let all = store.list(None, 10, 0).await.unwrap();
    assert!(all.is_empty());
}

#[tokio::test]
async fn test_remove_by_message() {
    let store = create_test_store().await;
    let input = make_input("bye");
    store.add(input.clone()).await.unwrap();

    store
        .remove_by_message(&input.session_id, &input.message_id)
        .await
        .unwrap();
    let fetched = store
        .get_by_message(&input.session_id, &input.message_id)
        .await
        .unwrap();
    assert!(fetched.is_none());
}

#[tokio::test]
async fn test_list_search_and_pagination() {
    let store = create_test_store().await;
    for i in 0..5 {
        let mut input = make_input(&format!("answer {i}"));
        if i == 3 {
            input.note = Some("rust tips".to_string());
        }
        store.add(input).await.unwrap();
    }

    let all = store.list(None, 10, 0).await.unwrap();
    assert_eq!(all.len(), 5);

    let page = store.list(None, 2, 2).await.unwrap();
    assert_eq!(page.len(), 2);

    let by_content = store.list(Some("answer 1"), 10, 0).await.unwrap();
    assert_eq!(by_content.len(), 1);

    let by_note = store.list(Some("rust"), 10, 0).await.unwrap();
    assert_eq!(by_note.len(), 1);
    assert_eq!(by_note[0].note.as_deref(), Some("rust tips"));

    let by_title = store.list(Some("Test session"), 10, 0).await.unwrap();
    assert_eq!(by_title.len(), 5);

    let none = store.list(Some("nonexistent"), 10, 0).await.unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn test_list_search_escapes_like_wildcards() {
    let store = create_test_store().await;
    store
        .add(make_input("progress is 100% done"))
        .await
        .unwrap();
    store
        .add(make_input("progress is 1000 done"))
        .await
        .unwrap();

    // "%" must be treated literally, not as a LIKE wildcard.
    let hits = store.list(Some("100%"), 10, 0).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].content, "progress is 100% done");

    let hits = store.list(Some("100_"), 10, 0).await.unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn test_add_same_message_preserves_message_created_at() {
    let store = create_test_store().await;
    let mut input = make_input("v1");
    let created = chrono::Utc::now();
    input.message_created_at = Some(created);
    store.add(input.clone()).await.unwrap();

    // Re-favoriting without a timestamp keeps the original one.
    input.message_created_at = None;
    let updated = store.add(input.clone()).await.unwrap();
    assert_eq!(updated.message_created_at, Some(created));
}

#[tokio::test]
async fn test_update_note() {
    let store = create_test_store().await;
    let added = store.add(make_input("noted")).await.unwrap();

    store.update_note(&added.id, Some("my note")).await.unwrap();
    let fetched = store
        .get_by_message(&added.session_id, &added.message_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.note.as_deref(), Some("my note"));

    store.update_note(&added.id, None).await.unwrap();
    let fetched = store
        .get_by_message(&added.session_id, &added.message_id)
        .await
        .unwrap()
        .unwrap();
    assert!(fetched.note.is_none());

    let err = store.update_note("fav_missing", Some("x")).await;
    assert!(err.is_err());
}
