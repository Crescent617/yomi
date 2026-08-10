use super::*;

use sqlx::sqlite::SqlitePoolOptions;

async fn create_test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::storage::migrations::run_migrations(&pool)
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
            mapping_key: "12345".to_string(),
            doc_comment: None,
        })
    );

    let not_found = store
        .find_routing_by_session(&SessionId::new())
        .await
        .unwrap();
    assert_eq!(not_found, None);
}

#[tokio::test]
async fn test_find_routing_parses_doc_comment_mapping_key() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    let sid = SessionId::new();
    // Doc-comment sessions store an empty actual chat id; the delivery
    // target rides the mapping key (see `doc_comment_mapping_key`).
    store
        .save_mapping("feishu", "doc:docx:tok123:c_1", &sid, "", None)
        .await
        .unwrap();

    let found = store.find_routing_by_session(&sid).await.unwrap();
    assert_eq!(
        found,
        Some(SessionRouting {
            channel_name: "feishu".to_string(),
            external_chat_id: String::new(),
            reply_msg_id: None,
            mapping_key: "doc:docx:tok123:c_1".to_string(),
            doc_comment: Some(crate::channels::DocCommentRef {
                file_token: "tok123".to_string(),
                file_type: "docx".to_string(),
                comment_id: "c_1".to_string(),
            }),
        })
    );
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

#[tokio::test]
async fn test_history_cursor_round_trip() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        None
    );

    store
        .set_history_cursor("feishu", "oc_1", 1_700_000_060_000)
        .await
        .unwrap();
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        Some(1_700_000_060_000)
    );

    // Upsert advances; containers and channels are independent keys.
    store
        .set_history_cursor("feishu", "oc_1", 1_700_000_120_000)
        .await
        .unwrap();
    store
        .set_history_cursor("feishu", "omt_1", 42)
        .await
        .unwrap();
    store
        .set_history_cursor("telegram", "oc_1", 7)
        .await
        .unwrap();
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        Some(1_700_000_120_000)
    );
    assert_eq!(
        store.get_history_cursor("feishu", "omt_1").await.unwrap(),
        Some(42)
    );
    assert_eq!(
        store.get_history_cursor("telegram", "oc_1").await.unwrap(),
        Some(7)
    );
}

#[tokio::test]
async fn test_mention_override_round_trip() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    assert_eq!(
        store.get_mention_override("feishu", "oc_1").await.unwrap(),
        None
    );

    store
        .set_mention_override("feishu", "oc_1", false)
        .await
        .unwrap();
    assert_eq!(
        store.get_mention_override("feishu", "oc_1").await.unwrap(),
        Some(false)
    );

    // Upsert replaces; containers and channels are independent keys.
    store
        .set_mention_override("feishu", "oc_1", true)
        .await
        .unwrap();
    store
        .set_mention_override("feishu", "omt_1", false)
        .await
        .unwrap();
    store
        .set_mention_override("telegram", "oc_1", false)
        .await
        .unwrap();
    assert_eq!(
        store.get_mention_override("feishu", "oc_1").await.unwrap(),
        Some(true)
    );
    assert_eq!(
        store.get_mention_override("feishu", "omt_1").await.unwrap(),
        Some(false)
    );

    // Clear removes the row; other keys are untouched.
    store
        .clear_mention_override("feishu", "oc_1")
        .await
        .unwrap();
    assert_eq!(
        store.get_mention_override("feishu", "oc_1").await.unwrap(),
        None
    );
    assert_eq!(
        store.get_mention_override("feishu", "omt_1").await.unwrap(),
        Some(false)
    );
    // Clearing a missing key is a no-op.
    store
        .clear_mention_override("feishu", "oc_1")
        .await
        .unwrap();
}

#[tokio::test]
async fn test_rit_override_round_trip() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    assert_eq!(
        store.get_rit_override("feishu", "oc_1").await.unwrap(),
        None
    );

    store
        .set_rit_override("feishu", "oc_1", true)
        .await
        .unwrap();
    assert_eq!(
        store.get_rit_override("feishu", "oc_1").await.unwrap(),
        Some(true)
    );

    // Upsert replaces; chats and channels are independent keys.
    store
        .set_rit_override("feishu", "oc_1", false)
        .await
        .unwrap();
    store
        .set_rit_override("feishu", "oc_2", true)
        .await
        .unwrap();
    store
        .set_rit_override("telegram", "oc_1", true)
        .await
        .unwrap();
    assert_eq!(
        store.get_rit_override("feishu", "oc_1").await.unwrap(),
        Some(false)
    );
    assert_eq!(
        store.get_rit_override("feishu", "oc_2").await.unwrap(),
        Some(true)
    );

    // Clear removes the row; other keys are untouched.
    store.clear_rit_override("feishu", "oc_1").await.unwrap();
    assert_eq!(
        store.get_rit_override("feishu", "oc_1").await.unwrap(),
        None
    );
    assert_eq!(
        store.get_rit_override("feishu", "oc_2").await.unwrap(),
        Some(true)
    );
    // Clearing a missing key is a no-op.
    store.clear_rit_override("feishu", "oc_1").await.unwrap();
}

// ── Doc permission requests ────────────────────────────────────────

fn perm_req() -> DocPermissionRequest {
    DocPermissionRequest {
        file_token: "doxcnABC".to_string(),
        file_type: "docx".to_string(),
        permission: "view".to_string(),
        remark: Some("求权限".to_string()),
        applicant_users: vec!["ou_aaa".to_string()],
        applicant_chats: vec!["oc_bbb".to_string()],
        applicant_departments: vec![],
    }
}

#[tokio::test]
async fn perm_request_save_dedups_pending_duplicates() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    let id = store
        .save_perm_request("feishu", &perm_req())
        .await
        .unwrap();
    assert!(id.is_some());

    // Same application still pending → dedup hit (ws redelivery).
    let dup = store
        .save_perm_request("feishu", &perm_req())
        .await
        .unwrap();
    assert_eq!(dup, None);

    // A different permission level is a different application.
    let mut edit_req = perm_req();
    edit_req.permission = "edit".to_string();
    let id2 = store.save_perm_request("feishu", &edit_req).await.unwrap();
    assert!(id2.is_some() && id2 != id);

    // Once resolved, a fresh application for the same file is accepted.
    store
        .resolve_perm_request(id.unwrap(), "approved", "ou_admin", None)
        .await
        .unwrap();
    let id3 = store
        .save_perm_request("feishu", &perm_req())
        .await
        .unwrap();
    assert!(id3.is_some());
}

#[tokio::test]
async fn perm_request_resolve_wins_once_and_reopen_restores() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);
    let id = store
        .save_perm_request("feishu", &perm_req())
        .await
        .unwrap()
        .unwrap();

    let row = store
        .resolve_perm_request(id, "approved", "ou_admin", Some("edit"))
        .await
        .unwrap()
        .expect("first resolve wins");
    assert_eq!(row.status, "approved");
    assert_eq!(row.resolved_by.as_deref(), Some("ou_admin"));
    assert_eq!(row.resolved_perm.as_deref(), Some("edit"));
    assert_eq!(row.applicant_users, vec!["ou_aaa".to_string()]);
    assert_eq!(row.applicant_chats, vec!["oc_bbb".to_string()]);

    // Second resolve loses the race — concurrent approvals run once.
    let lost = store
        .resolve_perm_request(id, "denied", "ou_other", None)
        .await
        .unwrap();
    assert!(lost.is_none());

    // Reopen (grant API failed): back to pending, resolvable again.
    store.reopen_perm_request(id).await.unwrap();
    let pending = store.list_pending_perm_requests("feishu").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].resolved_by, None);
    assert_eq!(pending[0].resolved_perm, None);

    let row = store
        .resolve_perm_request(id, "denied", "ou_admin", None)
        .await
        .unwrap()
        .expect("resolvable after reopen");
    assert_eq!(row.status, "denied");
}

#[tokio::test]
async fn perm_request_list_pending_and_notify_msgs() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    let id1 = store
        .save_perm_request("feishu", &perm_req())
        .await
        .unwrap()
        .unwrap();
    let mut other = perm_req();
    other.file_token = "doxcnOTHER".to_string();
    let id2 = store
        .save_perm_request("feishu", &other)
        .await
        .unwrap()
        .unwrap();
    // Another channel's rows don't leak in.
    store.save_perm_request("lark", &perm_req()).await.unwrap();

    store
        .set_perm_notify_msgs(id1, &["om_x".to_string(), "om_y".to_string()])
        .await
        .unwrap();

    let rows = store.list_pending_perm_requests("feishu").await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, id1, "oldest first");
    assert_eq!(rows[1].id, id2);
    assert_eq!(rows[0].notify_msg_ids, vec!["om_x", "om_y"]);
    assert!(rows[1].notify_msg_ids.is_empty());
    assert_eq!(rows[0].remark.as_deref(), Some("求权限"));
    assert_eq!(rows[0].file_type, "docx");
    assert!(!rows[0].created_at.is_empty());

    // Resolved rows leave the pending list.
    store
        .resolve_perm_request(id1, "approved", "ou_admin", None)
        .await
        .unwrap();
    let rows = store.list_pending_perm_requests("feishu").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id2);
}

/// Run subscriptions: upsert on (channel, scope, subscriber), exact +
/// recursive matching, and removal.
#[tokio::test]
async fn test_run_subscription_round_trip_and_matching() {
    let pool = create_test_pool().await;
    let store = SqliteChannelStore::new(pool);

    // DM subscription on the chat scope.
    store
        .save_run_subscription("feishu", "oc_1", "oc_1", false, "ou_a", None)
        .await
        .unwrap();
    // Recursive chat subscription.
    store
        .save_run_subscription("feishu", "oc_1", "oc_1", true, "ou_b", None)
        .await
        .unwrap();
    // Thread subscription (scope = thread key) with a target chat.
    store
        .save_run_subscription("feishu", "omt_1", "oc_1", false, "ou_c", Some("oc_2"))
        .await
        .unwrap();
    // Another channel — never matches.
    store
        .save_run_subscription("other", "oc_1", "oc_1", false, "ou_d", None)
        .await
        .unwrap();

    // Chat-level run: exact + recursive match.
    let subs = store
        .list_matching_run_subscriptions("feishu", "oc_1", "oc_1")
        .await
        .unwrap();
    let mut ids: Vec<_> = subs.iter().map(|s| s.subscriber_open_id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["ou_a", "ou_b"]);

    // Thread run in the same chat: exact thread sub + recursive chat sub;
    // the non-recursive chat sub stays out.
    let subs = store
        .list_matching_run_subscriptions("feishu", "omt_1", "oc_1")
        .await
        .unwrap();
    let mut ids: Vec<_> = subs.iter().map(|s| s.subscriber_open_id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["ou_b", "ou_c"]);
    let thread_sub = subs
        .iter()
        .find(|s| s.subscriber_open_id == "ou_c")
        .unwrap();
    assert_eq!(thread_sub.target_chat_id.as_deref(), Some("oc_2"));
    assert!(!thread_sub.created_at.is_empty());

    // A run in a different chat matches nothing.
    assert!(store
        .list_matching_run_subscriptions("feishu", "oc_9", "oc_9")
        .await
        .unwrap()
        .is_empty());

    // Upsert: re-subscribing flips recursive/target, no duplicate row.
    store
        .save_run_subscription("feishu", "oc_1", "oc_1", false, "ou_b", Some("oc_3"))
        .await
        .unwrap();
    let subs = store
        .list_matching_run_subscriptions("feishu", "omt_9", "oc_1")
        .await
        .unwrap();
    assert!(
        subs.iter().all(|s| s.subscriber_open_id != "ou_b"),
        "no longer recursive"
    );
    let subs = store
        .list_matching_run_subscriptions("feishu", "oc_1", "oc_1")
        .await
        .unwrap();
    assert_eq!(subs.len(), 2);

    // Removal.
    let removed = store
        .remove_run_subscription("feishu", "oc_1", "ou_a")
        .await
        .unwrap();
    assert_eq!(removed, 1);
    let removed = store
        .remove_run_subscription("feishu", "oc_1", "ou_a")
        .await
        .unwrap();
    assert_eq!(removed, 0);
}
