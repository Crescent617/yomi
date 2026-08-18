use super::*;
use crate::channels::store::SqliteChannelStore;
use crate::channels::ChannelError;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Mutex;

// ── Fixtures ───────────────────────────────────────────────────────

async fn test_store() -> Arc<dyn ChannelStore> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::storage::migrations::run_migrations(&pool)
        .await
        .unwrap();
    Arc::new(SqliteChannelStore::new(pool))
}

#[derive(Default)]
struct MockAdapter {
    granted: Mutex<Vec<(String, String, String)>>, // token, file_type, perm
    updated_cards: Mutex<Vec<(String, String)>>,   // msg_id, json
    sent_cards: Mutex<Vec<(String, String)>>,      // chat_id, json
    direct_cards: Mutex<Vec<(String, String)>>,    // user_id, json
    sent_messages: Mutex<Vec<(String, String)>>,   // chat_id, text
    fail_grant: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: tokio::sync::mpsc::Sender<crate::channels::ChannelEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        external_chat_id: &str,
        blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        let text = super::super::blocks_to_text(&blocks);
        self.sent_messages
            .lock()
            .unwrap()
            .push((external_chat_id.to_string(), text));
        Ok(None)
    }

    async fn send_card(
        &self,
        external_chat_id: &str,
        card_json: &str,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        self.sent_cards
            .lock()
            .unwrap()
            .push((external_chat_id.to_string(), card_json.to_string()));
        Ok(Some(format!(
            "om_card_{}",
            self.sent_cards.lock().unwrap().len()
        )))
    }

    async fn send_direct_card(
        &self,
        user_id: &str,
        card_json: &str,
    ) -> Result<Option<String>, ChannelError> {
        self.direct_cards
            .lock()
            .unwrap()
            .push((user_id.to_string(), card_json.to_string()));
        Ok(Some(format!(
            "om_dm_{}",
            self.direct_cards.lock().unwrap().len()
        )))
    }

    async fn update_card(&self, message_id: &str, card_json: &str) -> Result<(), ChannelError> {
        self.updated_cards
            .lock()
            .unwrap()
            .push((message_id.to_string(), card_json.to_string()));
        Ok(())
    }

    async fn grant_doc_permission(
        &self,
        file_token: &str,
        file_type: &str,
        _req: &DocPermissionRequest,
        perm: &str,
    ) -> Result<(), ChannelError> {
        if self.fail_grant.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(ChannelError::Platform("grant exploded".into()));
        }
        self.granted.lock().unwrap().push((
            file_token.to_string(),
            file_type.to_string(),
            perm.to_string(),
        ));
        Ok(())
    }
}

fn admin_config() -> ChannelConfig {
    ChannelConfig {
        admin_users: vec!["ou_admin".to_string()],
        ..ChannelConfig::default()
    }
}

fn perm_req() -> DocPermissionRequest {
    DocPermissionRequest {
        file_token: "doxcnABC".to_string(),
        file_type: "docx".to_string(),
        permission: "view".to_string(),
        remark: Some("求权限".to_string()),
        applicant_users: vec!["ou_aaa".to_string()],
        applicant_chats: vec![],
        applicant_departments: vec![],
    }
}

async fn seed_pending(store: &Arc<dyn ChannelStore>) -> i64 {
    store
        .save_perm_request("feishu", &perm_req())
        .await
        .unwrap()
        .unwrap()
}

fn card_action(operator: &str, value: serde_json::Value) -> CardAction {
    CardAction {
        operator_open_id: operator.to_string(),
        chat_id: Some("oc_chat".to_string()),
        message_id: None,
        value,
    }
}

/// The button-failure feedback rides a spawned task — poll briefly.
async fn wait_for_messages(adapter: &MockAdapter) -> Vec<(String, String)> {
    for _ in 0..100 {
        let msgs = adapter.sent_messages.lock().unwrap().clone();
        if !msgs.is_empty() {
            return msgs;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Vec::new()
}

// ── Card building ──────────────────────────────────────────────────

#[test]
fn request_card_carries_button_values() {
    let card = build_request_card(7, &perm_req(), Some("测试文档"));
    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    assert_eq!(v["header"]["template"], "orange");
    assert_eq!(v["header"]["title"]["content"], "📄 文档权限申请 #7");

    // Buttons sit on one row inside a column_set.
    let columns = &v["body"]["elements"][1]["columns"];
    let approve_btn = &columns[0]["elements"][0];
    assert_eq!(approve_btn["tag"], "button");
    assert_eq!(
        approve_btn["behaviors"][0]["value"],
        json!({ "action": "approve", "id": 7 })
    );
    let deny_btn = &columns[1]["elements"][0];
    assert_eq!(deny_btn["tag"], "button");
    assert_eq!(
        deny_btn["behaviors"][0]["value"],
        json!({ "action": "deny", "id": 7 })
    );

    let md = v["body"]["elements"][0]["content"].as_str().unwrap();
    assert!(md.contains("<at id=ou_aaa></at>"), "{md}");
    assert!(
        md.contains("[测试文档](https://feishu.cn/docx/doxcnABC)"),
        "{md}"
    );
    assert!(md.contains("**申请权限** view"), "{md}");
    assert!(md.contains("**备注** 求权限"), "{md}");

    let hint = v["body"]["elements"][2]["content"].as_str().unwrap();
    assert!(hint.contains("/approve 7"), "{hint}");
}

#[tokio::test]
async fn resolved_card_shows_terminal_state_without_buttons() {
    let store = test_store().await;
    let id = seed_pending(&store).await;
    let row = store
        .resolve_perm_request(id, "approved", "ou_admin", Some("edit"))
        .await
        .unwrap()
        .unwrap();

    let card = build_resolved_card(&row, None);
    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    assert_eq!(v["header"]["template"], "green");
    let md = v["body"]["elements"][0]["content"].as_str().unwrap();
    assert!(md.contains("已批准 edit"), "{md}");
    assert!(md.contains("by <at id=ou_admin></at>"), "{md}");
    assert_eq!(
        v["body"]["elements"].as_array().unwrap().len(),
        1,
        "no buttons"
    );
}

// ── Formatting ─────────────────────────────────────────────────────

#[test]
fn applicants_formatting() {
    let users = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        format_applicants(&users(&["ou_a"]), &[], &[]),
        "<at id=ou_a></at>"
    );
    assert_eq!(
        format_applicants(&users(&["ou_a", "ou_b"]), &[], &[]),
        "<at id=ou_a></at>（等 2 人）"
    );
    assert_eq!(
        format_applicants(&users(&["ou_a"]), &users(&["oc_c"]), &users(&["od_d"])),
        "<at id=ou_a></at> · 群 oc_c · 部门 od_d"
    );
    assert_eq!(format_applicants(&[], &[], &[]), "");
}

#[tokio::test]
async fn pending_list_formatting() {
    let store = test_store().await;
    assert_eq!(format_pending_list(&[]), "没有待审批的文档权限申请。");

    seed_pending(&store).await;
    let rows = store.list_pending_perm_requests("feishu").await.unwrap();
    let text = format_pending_list(&rows);
    assert!(text.contains("ou_aaa"), "{text}");
    assert!(text.contains("docx/doxcnABC"), "{text}");
    assert!(text.contains("/approve <id>"), "{text}");
}

// ── Commands ───────────────────────────────────────────────────────

#[tokio::test]
async fn permits_requires_admin() {
    let reply = check_admin(&admin_config(), "ou_stranger");
    assert_eq!(
        reply.as_deref(),
        Some("permission denied：你不在 admin_users 中。")
    );
}

#[tokio::test]
async fn approve_rejects_invalid_perm() {
    let store = test_store().await;
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(MockAdapter::default());
    let id = seed_pending(&store).await;

    let reply = approve(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        "ou_admin",
        id,
        Some("owner"),
    )
    .await
    .unwrap();
    assert!(reply.unwrap().contains("无效权限级别"));
    // Still pending afterwards.
    assert_eq!(
        store
            .list_pending_perm_requests("feishu")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn approve_grants_and_updates_cards() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;
    store
        .set_perm_notify_msgs(id, &["om_n1".to_string(), "om_n2".to_string()])
        .await
        .unwrap();

    let reply = approve(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        "ou_admin",
        id,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(reply.contains("已批准"), "{reply}");
    assert!(reply.contains("view"), "{reply}");

    // Granted with the requested level; both notify cards updated.
    assert_eq!(
        mock.granted.lock().unwrap().as_slice(),
        &[(
            "doxcnABC".to_string(),
            "docx".to_string(),
            "view".to_string()
        )]
    );
    let updated = mock.updated_cards.lock().unwrap().clone();
    assert_eq!(updated.len(), 2);
    assert_eq!(updated[0].0, "om_n1");
    assert!(updated[0].1.contains("已批准 view"));

    // Row left pending state.
    assert!(store
        .list_pending_perm_requests("feishu")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn approve_with_perm_override() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;

    approve(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        "ou_admin",
        id,
        Some("full_access"),
    )
    .await
    .unwrap();
    assert_eq!(
        mock.granted.lock().unwrap().as_slice(),
        &[(
            "doxcnABC".to_string(),
            "docx".to_string(),
            "full_access".to_string()
        )]
    );
}

#[tokio::test]
async fn approve_twice_second_loses() {
    let store = test_store().await;
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(MockAdapter::default());
    let config = ChannelConfig {
        admin_users: vec!["ou_admin".to_string(), "ou_admin2".to_string()],
        ..ChannelConfig::default()
    };
    let id = seed_pending(&store).await;

    approve("feishu", &config, &store, &adapter, "ou_admin", id, None)
        .await
        .unwrap();
    let second = approve("feishu", &config, &store, &adapter, "ou_admin2", id, None)
        .await
        .unwrap()
        .unwrap();
    assert!(second.contains("不存在或已被处理"), "{second}");
}

#[tokio::test]
async fn approve_grant_failure_reopens() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    mock.fail_grant
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;

    let reply = approve(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        "ou_admin",
        id,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(reply.contains("失败"), "{reply}");
    assert!(reply.contains("恢复为待审批"), "{reply}");

    // Back to pending — resolvable again.
    assert_eq!(
        store
            .list_pending_perm_requests("feishu")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn approve_unrecognized_stored_perm_reopens() {
    // R6: a future/unknown level in the stored request must not be granted
    // blindly — the request reopens and asks for an explicit level.
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut req = perm_req();
    req.permission = "comment_v2".to_string();
    let id = store
        .save_perm_request("feishu", &req)
        .await
        .unwrap()
        .unwrap();

    let reply = approve(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        "ou_admin",
        id,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(reply.contains("无法识别"), "{reply}");
    assert!(mock.granted.lock().unwrap().is_empty(), "no blind grant");
    assert_eq!(
        store
            .list_pending_perm_requests("feishu")
            .await
            .unwrap()
            .len(),
        1,
        "reopened for an explicit /approve <id> <perm>"
    );
}

#[tokio::test]
async fn deny_marks_denied_without_grant() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;
    store
        .set_perm_notify_msgs(id, &["om_n1".to_string()])
        .await
        .unwrap();

    let reply = deny("feishu", &admin_config(), &store, &adapter, "ou_admin", id)
        .await
        .unwrap()
        .unwrap();
    assert!(reply.contains("已拒绝"), "{reply}");
    assert!(mock.granted.lock().unwrap().is_empty(), "deny never grants");
    let updated = mock.updated_cards.lock().unwrap();
    assert_eq!(updated.len(), 1);
    assert!(updated[0].1.contains("已拒绝"));
}

// ── Button callbacks ───────────────────────────────────────────────

#[tokio::test]
async fn button_approve_resolves_without_feedback_message() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;
    store
        .set_perm_notify_msgs(id, &["om_n1".to_string()])
        .await
        .unwrap();

    handle_card_action(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        card_action("ou_admin", json!({ "action": "approve", "id": id })),
    )
    .await;

    assert_eq!(mock.granted.lock().unwrap().len(), 1);
    // Success speaks through the updated card — no chat message.
    assert!(
        wait_for_messages(&mock).await.is_empty(),
        "no feedback message on success"
    );
}

#[tokio::test]
async fn button_success_without_cards_falls_back_to_message() {
    // Notification cards were never recorded (R5) — the clicker must get
    // an explicit message, or a successful approval is invisible.
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;

    handle_card_action(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        card_action("ou_admin", json!({ "action": "approve", "id": id })),
    )
    .await;

    assert_eq!(mock.granted.lock().unwrap().len(), 1);
    let msgs = wait_for_messages(&mock).await;
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].1.contains("已批准"), "{}", msgs[0].1);
}

#[tokio::test]
async fn button_deny_resolves() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;

    handle_card_action(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        card_action("ou_admin", json!({ "action": "deny", "id": id })),
    )
    .await;

    assert!(mock.granted.lock().unwrap().is_empty());
    assert!(store
        .list_pending_perm_requests("feishu")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn button_non_admin_gets_feedback_message() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;

    handle_card_action(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        card_action("ou_stranger", json!({ "action": "approve", "id": id })),
    )
    .await;

    let msgs = wait_for_messages(&mock).await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].0, "oc_chat");
    assert!(msgs[0].1.contains("permission denied"), "{}", msgs[0].1);
    // Nothing was resolved.
    assert_eq!(
        store
            .list_pending_perm_requests("feishu")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn button_repeat_click_reports_already_resolved() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let id = seed_pending(&store).await;
    store
        .set_perm_notify_msgs(id, &["om_n1".to_string()])
        .await
        .unwrap();

    let action = card_action("ou_admin", json!({ "action": "approve", "id": id }));
    handle_card_action("feishu", &admin_config(), &store, &adapter, action.clone()).await;
    handle_card_action("feishu", &admin_config(), &store, &adapter, action).await;

    let msgs = wait_for_messages(&mock).await;
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].1.contains("已被处理"), "{}", msgs[0].1);
    assert_eq!(
        mock.granted.lock().unwrap().len(),
        1,
        "granted exactly once"
    );
}

#[tokio::test]
async fn button_unknown_value_is_ignored() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    handle_card_action(
        "feishu",
        &admin_config(),
        &store,
        &adapter,
        card_action("ou_admin", json!({ "action": "explode" })),
    )
    .await;
    assert!(mock.granted.lock().unwrap().is_empty());
    assert!(wait_for_messages(&mock).await.is_empty());
}

// ── Notification delivery ──────────────────────────────────────────

#[tokio::test]
async fn notify_group_card_when_approval_chat_configured() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig {
        approval_chat_id: Some("oc_admin_group".to_string()),
        ..admin_config()
    };

    handle_doc_permission_applied("feishu", &config, &store, &adapter, perm_req()).await;

    assert_eq!(mock.sent_cards.lock().unwrap().len(), 1);
    assert_eq!(mock.sent_cards.lock().unwrap()[0].0, "oc_admin_group");
    assert!(
        mock.direct_cards.lock().unwrap().is_empty(),
        "no DM in group mode"
    );
    // The card's message id is recorded for later updates.
    let rows = store.list_pending_perm_requests("feishu").await.unwrap();
    assert_eq!(rows[0].notify_msg_ids, vec!["om_card_1".to_string()]);
}

#[tokio::test]
async fn notify_dms_every_admin_without_approval_chat() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig {
        admin_users: vec!["ou_a1".to_string(), "ou_a2".to_string()],
        ..ChannelConfig::default()
    };

    handle_doc_permission_applied("feishu", &config, &store, &adapter, perm_req()).await;

    let dms = mock.direct_cards.lock().unwrap().clone();
    assert_eq!(dms.len(), 2);
    assert_eq!(dms[0].0, "ou_a1");
    assert_eq!(dms[1].0, "ou_a2");
    assert!(dms[0].1.contains("文档权限申请 #1"), "{}", dms[0].1);
    let rows = store.list_pending_perm_requests("feishu").await.unwrap();
    assert_eq!(rows[0].notify_msg_ids.len(), 2);
}

#[tokio::test]
async fn notify_skipped_when_feature_unconfigured() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default(); // no approval_chat_id, no admin_users

    handle_doc_permission_applied("feishu", &config, &store, &adapter, perm_req()).await;

    assert!(mock.sent_cards.lock().unwrap().is_empty());
    assert!(mock.direct_cards.lock().unwrap().is_empty());
    // Row still recorded — approvable via /permits later.
    assert_eq!(
        store
            .list_pending_perm_requests("feishu")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn duplicate_event_records_once_and_notifies_once() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = admin_config();

    handle_doc_permission_applied("feishu", &config, &store, &adapter, perm_req()).await;
    handle_doc_permission_applied("feishu", &config, &store, &adapter, perm_req()).await;

    assert_eq!(
        store
            .list_pending_perm_requests("feishu")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        mock.direct_cards.lock().unwrap().len(),
        1,
        "ws redelivery doesn't re-notify"
    );
}
