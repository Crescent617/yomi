use super::*;

use sqlx::sqlite::SqlitePoolOptions;

async fn create_test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        r"CREATE TABLE channel_session_mappings (
                channel_name TEXT NOT NULL,
                external_chat_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                actual_chat_id TEXT NOT NULL,
                reply_msg_id TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (channel_name, external_chat_id)
            );
            CREATE INDEX idx_channel_mapping_session ON channel_session_mappings(session_id);",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn test_save_and_find_mapping() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    let sid = SessionId::new();
    store
        .save_mapping("tg_bot", "12345", &sid, "chat123", None)
        .await
        .unwrap();

    let found = store.find_mapping("tg_bot", "12345").await.unwrap();
    assert_eq!(found, Some(sid));

    let not_found = store.find_mapping("tg_bot", "99999").await.unwrap();
    assert_eq!(not_found, None);
}

#[tokio::test]
async fn test_update_mapping() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    let sid1 = SessionId::new();
    let sid2 = SessionId::new();
    store
        .save_mapping("tg_bot", "12345", &sid1, "chat123", None)
        .await
        .unwrap();
    store
        .save_mapping("tg_bot", "12345", &sid2, "chat123", None)
        .await
        .unwrap();

    let found = store.find_mapping("tg_bot", "12345").await.unwrap();
    assert_eq!(found, Some(sid2));
}

#[tokio::test]
async fn test_list_mappings() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    let sid1 = SessionId::new();
    let sid2 = SessionId::new();
    store
        .save_mapping("tg_bot", "111", &sid1, "chat1", None)
        .await
        .unwrap();
    store
        .save_mapping("tg_bot", "222", &sid2, "chat2", None)
        .await
        .unwrap();
    store
        .save_mapping("other", "333", &SessionId::new(), "chat3", None)
        .await
        .unwrap();

    let mappings = store.list_mappings("tg_bot").await.unwrap();
    assert_eq!(mappings.len(), 2);
}

#[tokio::test]
async fn test_find_routing_by_session_id() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    let sid = SessionId::new();
    store
        .save_mapping("tg_bot", "12345", &sid, "chat123", Some("root_msg"))
        .await
        .unwrap();

    let found = store.find_routing_by_session(&sid).await.unwrap();
    assert_eq!(
        found,
        Some(SessionRouting {
            channel_name: "tg_bot".to_string(),
            external_chat_id: "chat123".to_string(),
            reply_msg_id: Some("root_msg".to_string()),
        })
    );

    let not_found = store
        .find_routing_by_session(&SessionId::new())
        .await
        .unwrap();
    assert_eq!(not_found, None);
}

#[tokio::test]
async fn test_update_routing() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    let sid = SessionId::new();
    store
        .save_mapping("tg_bot", "thread1", &sid, "chat1", None)
        .await
        .unwrap();
    store
        .save_mapping("tg_bot", "thread1", &sid, "chat1", Some("msg2"))
        .await
        .unwrap();

    let found = store.find_routing_by_session(&sid).await.unwrap();
    assert_eq!(found.unwrap().reply_msg_id, Some("msg2".to_string()));
}
