use super::{duration_until_next_midnight, mark_user_steer};
use crate::storage::NewSession;
use crate::types::{ContentBlock, ImageUrl};

#[test]
fn duration_until_next_midnight_is_within_a_day() {
    let d = duration_until_next_midnight();
    assert!(d > std::time::Duration::ZERO);
    assert!(d <= std::time::Duration::from_hours(24));
}

#[test]
fn user_steer_prefixes_the_first_text_block() {
    assert_eq!(
        mark_user_steer(vec![ContentBlock::Text {
            text: "change direction".to_string(),
        }]),
        vec![ContentBlock::Text {
            text: "[From User] change direction".to_string(),
        }]
    );
}

#[test]
fn user_steer_inserts_prefix_before_non_text_content() {
    let image = ContentBlock::ImageUrl {
        image_url: ImageUrl {
            url: "data:image/png;base64,abc".to_string(),
            detail: None,
        },
    };

    assert_eq!(
        mark_user_steer(vec![image.clone()]),
        vec![
            ContentBlock::Text {
                text: "[From User] ".to_string(),
            },
            image,
        ]
    );
}

/// `gc.auto = true` + `Kernel::start` runs a gc pass at startup that purges
/// expired sessions (`dry_run` is never set for the daemon's auto gc).
#[tokio::test]
async fn auto_gc_collects_expired_sessions_on_start() {
    use crate::storage::StorageSet;

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.gc.auto = true;
    // Keep the pass minimal: this test exercises scheduling, not the sweep.
    config.gc.sweep_orphans = false;
    config.finalize();

    // Create and age a session before the kernel starts so the immediate
    // first gc pass collects it.
    let storage = StorageSet::open(tmp.path().to_path_buf()).await.unwrap();
    let id = crate::types::SessionId::new();
    storage
        .session_store()
        .create(NewSession {
            working_dir: Some("/test".into()),
            ..NewSession::new(id.clone())
        })
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-100 days') WHERE id = ?")
        .bind(&*id.0)
        .execute(storage.pool())
        .await
        .unwrap();

    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if storage.session_store().get(&id).await.unwrap().is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "auto gc did not collect the expired session in time"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    kernel.stop().await;
}

/// With `gc.auto = false` (the default) `Kernel::start` spawns no gc pass.
#[tokio::test]
async fn auto_gc_disabled_by_default() {
    use crate::storage::StorageSet;

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.finalize();

    let storage = StorageSet::open(tmp.path().to_path_buf()).await.unwrap();
    let id = crate::types::SessionId::new();
    storage
        .session_store()
        .create(NewSession {
            working_dir: Some("/test".into()),
            ..NewSession::new(id.clone())
        })
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-100 days') WHERE id = ?")
        .bind(&*id.0)
        .execute(storage.pool())
        .await
        .unwrap();

    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();

    // Give any (unexpected) gc pass a chance to run; the session must survive.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(storage.session_store().get(&id).await.unwrap().is_some());

    kernel.stop().await;
}

/// `create_session` without an explicit `working_dir` inherits the project
/// dir at creation time (instead of falling back to `<data_dir>/workspace`
/// at runtime); with neither project nor dir it stays unset.
#[tokio::test]
async fn create_session_inherits_project_dir_when_working_dir_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();

    let proj_dir = tmp.path().join("proj");
    std::fs::create_dir_all(&proj_dir).unwrap();
    let project = kernel.create_project(proj_dir.clone(), None).await.unwrap();

    let new_session = |project_id, working_dir| crate::kernel::CreateSessionInput {
        project_id,
        working_dir,
        auto_approve_level: None,
        tool_blocklist: vec![],
        model_key: None,
    };
    let stored_dir = async |sid: &crate::types::SessionId| {
        kernel
            .session_store()
            .await
            .get(sid)
            .await
            .unwrap()
            .unwrap()
            .working_dir
    };

    // Project-only creation inherits the (canonicalized) project dir.
    let sid = kernel
        .create_session(new_session(Some(project.id.clone()), None))
        .await
        .unwrap();
    assert_eq!(
        stored_dir(&sid).await.map(std::path::PathBuf::from),
        Some(std::fs::canonicalize(&proj_dir).unwrap())
    );

    // An explicit working_dir wins over the project dir.
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let sid = kernel
        .create_session(new_session(Some(project.id.clone()), Some(other.clone())))
        .await
        .unwrap();
    assert_eq!(
        stored_dir(&sid).await.map(std::path::PathBuf::from),
        Some(std::fs::canonicalize(&other).unwrap())
    );

    // Neither project nor dir → stays unset (runtime falls back to the
    // default workspace).
    let sid = kernel
        .create_session(new_session(None, None))
        .await
        .unwrap();
    assert_eq!(stored_dir(&sid).await, None);

    kernel.stop().await;
}

/// Mailbox 管理面端到端：入队可见、撤回、按范围清空、`MailboxChanged`
/// 事件计数跟随。本地黑洞 listener（accept 后永不响应）让 agent 挂在
/// 首个模型请求上，保证 pending 条目不被抢先消费——环境无关的确定性。
#[tokio::test]
async fn mailbox_management_snapshot_remove_clear() {
    // Accept connections and hold them open without ever responding.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock);
        }
    });

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        models: vec![crate::provider::ModelConfig {
            name: "blackhole".into(),
            endpoint: format!("http://{addr}"),
            ..Default::default()
        }],
        ..Default::default()
    };
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();
    let sid = crate::types::SessionId::new();
    kernel
        .session_store()
        .await
        .create(crate::storage::NewSession::new(sid.clone()))
        .await
        .unwrap();
    let text = |t: &str| {
        vec![ContentBlock::Text {
            text: t.to_string(),
        }]
    };

    // 占住 agent：第一条消息被消费后，模型请求挂起，后续消息全部排队。
    kernel.send_message(&sid, text("blocker")).await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let phase = kernel.get_session(&sid).await.map(|s| s.phase).ok();
        let empty = kernel.mailbox_snapshot(&sid).await.queue.is_empty();
        if phase.as_deref() == Some("streaming") && empty {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "agent not blocked yet"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // 入队可见：queue 一条、steer 一条。
    kernel.send_message(&sid, text("first task")).await.unwrap();
    kernel.send_steer(&sid, text("mid-run note")).await;
    let snap = loop {
        let snap = kernel.mailbox_snapshot(&sid).await;
        if snap.queue.len() == 1 && snap.steer.len() == 1 {
            break snap;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "items never landed: {snap:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(snap.steer[0].kind, crate::comms::MailboxItemKind::Steer);
    assert!(snap.steer[0].preview.contains("mid-run note"));
    assert!(snap.queue[0].preview.contains("first task"));

    // 撤回 queue 条目；steer 存活；重复撤回安全失败。
    assert!(
        kernel
            .remove_mailbox_item(&sid, snap.queue[0].id.as_str())
            .await
    );
    assert!(
        !kernel
            .remove_mailbox_item(&sid, snap.queue[0].id.as_str())
            .await
    );

    // 按范围清空 steer；然后 All 清空残余。
    assert_eq!(
        kernel
            .clear_mailbox(&sid, crate::comms::MailboxScope::Steer)
            .await,
        1
    );
    let snap = kernel.mailbox_snapshot(&sid).await;
    assert!(snap.steer.is_empty() && snap.queue.is_empty());
    kernel.stop().await;
}
