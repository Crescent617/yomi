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
        context_window: None,
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

/// fire 主路径（scheduler→worker→`Kernel::execute_cron_action`）：未绑定的
/// `send_message` job 每次执行新建独立会话（权限在建会话落点被钳到
/// caution、cwd 按模板继承）；绑定的 job 不新建任何会话。
#[tokio::test]
async fn cron_fire_per_run_spawns_fresh_session_and_clamps_level() {
    use crate::cron::{CronAction, CronExecutor, CronJob, CronJobStatus, CronSessionTemplate};
    use crate::storage::session::SessionListScope;

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();

    let make_job = |action| CronJob {
        id: crate::types::CronJobId::new(),
        name: "fire-test".to_string(),
        schedule: "0 9 * * *".to_string(),
        action,
        status: CronJobStatus::Active,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        next_run_at: None,
        last_run_at: None,
        run_count: 0,
        max_runs: 0,
        expires_at: crate::cron::NEVER_EXPIRES,
        last_error: None,
        precheck: None,
    };

    // 模板故意带 safe 级 + cwd：fire 时必须被钳到 caution，cwd 继承
    let per_run = make_job(CronAction::SendMessage {
        session_id: None,
        content: "wake {{date}}".to_string(),
        session_template: Some(CronSessionTemplate {
            working_dir: Some("/repo/demo".into()),
            project_id: None,
            auto_approve_level: Some("safe".into()),
        }),
    });
    kernel.execute_cron_action(&per_run).await.unwrap();
    kernel.execute_cron_action(&per_run).await.unwrap();

    let store = kernel.session_store().await;
    let (sessions, _) = store
        .list(None, SessionListScope::All, None, 10)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 2, "each fire spawns one fresh session");
    for s in &sessions {
        assert!(s.title.as_deref().unwrap().starts_with("fire-test · "));
        assert_eq!(s.auto_approve_level.as_deref(), Some("caution"));
        assert_eq!(s.working_dir.as_deref(), Some("/repo/demo"));
    }

    // 绑定的 job：消息直接投递，不新建任何会话
    let bound = make_job(CronAction::SendMessage {
        session_id: Some("sess-fixed".to_string()),
        content: "hi".to_string(),
        session_template: None,
    });
    kernel.execute_cron_action(&bound).await.unwrap();
    let (sessions, _) = store
        .list(None, SessionListScope::All, None, 10)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 2, "bound fire spawns nothing");

    kernel.stop().await;
}

/// fork 连同 per-session rules 一起复制：rules 是 spawn 时注入的
/// prompt 组成部分，fork 丢失它等于静默改变新会话的行为。无 rules 的
/// parent 则不落文件、不报错。
#[tokio::test]
async fn fork_session_copies_rules_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();

    let rules_dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();

    let new_bare_session = || async {
        let id = crate::types::SessionId::new();
        kernel
            .session_store()
            .await
            .create(crate::storage::NewSession::new(id.clone()))
            .await
            .unwrap();
        id
    };

    // 有 rules：fork 后 child 拿到同内容副本。
    let parent = new_bare_session().await;
    std::fs::write(rules_dir.join(format!("{parent}.md")), "用中文回答。").unwrap();
    let child = kernel
        .fork_session(&parent, crate::permission::Level::Caution)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(rules_dir.join(format!("{child}.md"))).unwrap(),
        "用中文回答。"
    );

    // 无 rules：fork 正常完成，不落 rules 文件。
    let bare = new_bare_session().await;
    let child = kernel
        .fork_session(&bare, crate::permission::Level::Caution)
        .await
        .unwrap();
    assert!(!rules_dir.join(format!("{child}.md")).exists());

    kernel.stop().await;
}

/// `get_session_rules`：channel 层经 routing 行解析 chat id 再读文件
/// （thread 行的 actual_chat_id 已 denormalize 父群 id，无需 thread→chat
/// 反查），session 层按会话 id 读；sub-agent 永不返回 session 层——与
/// spawn 时不注入对齐，视图不能展示下次 spawn 不会注入的规则。
#[tokio::test]
async fn get_session_rules_layers_and_sub_agent_exclusion() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        channels: vec![crate::channels::ChannelConfig {
            name: "mock".to_string(),
            enabled: true,
            platform: crate::channels::PlatformConfig::Feishu {
                app_id: "app".to_string(),
                app_secret: "secret".to_string(),
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();

    // Thread-scoped routing row: mapping key is the thread, actual chat
    // id the parent chat — rules resolve against the chat.
    let sid = crate::types::SessionId::new();
    kernel
        .channel_manager()
        .expect("channel hub")
        .store()
        .save_mapping(
            "mock",
            "omt_1",
            &sid,
            "oc_1",
            None,
            crate::channels::MappingKind::Normal,
        )
        .await
        .unwrap();

    let channel_dir = tmp.path().join("channels").join("rules");
    std::fs::create_dir_all(&channel_dir).unwrap();
    std::fs::write(channel_dir.join("oc_1.md"), "用中文回答。").unwrap();
    let session_dir = tmp.path().join("sessions").join("rules");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(session_dir.join(format!("{sid}.md")), "只答本话题。").unwrap();

    let rules = kernel.get_session_rules(&sid).await.unwrap();
    assert_eq!(rules.chat_id.as_deref(), Some("oc_1"));
    assert_eq!(rules.channel_rules.as_deref(), Some("用中文回答。"));
    assert_eq!(rules.session_rules.as_deref(), Some("只答本话题。"));

    // Sub-agent：即使存在同名 rules 文件、甚至存在 routing 行，两层都不
    // 返回（conductor 的 !is_sub_agent 哨兵在此同构）。
    let sub = crate::types::SessionId::from(format!("{}xyz", crate::types::SUB_PREFIX));
    std::fs::write(session_dir.join(format!("{sub}.md")), "不该显示").unwrap();
    kernel
        .channel_manager()
        .expect("channel hub")
        .store()
        .save_mapping(
            "mock",
            "omt_sub",
            &sub,
            "oc_1",
            None,
            crate::channels::MappingKind::Normal,
        )
        .await
        .unwrap();
    let rules = kernel.get_session_rules(&sub).await.unwrap();
    assert_eq!(rules.session_rules, None);
    assert_eq!(rules.chat_id, None);
    assert_eq!(rules.channel_rules, None);

    kernel.stop().await;
}

/// `get/set_session_context_window`：覆盖 → 生效值与来源正确；清除 →
/// 回落模型默认；`Some(0)` 拒绝；未知 session 报 NotFound；换模型
/// **不清**覆盖（model_default 跟随新模型）；create 传 Some(0) 过滤为
/// 无覆盖。
#[tokio::test]
async fn session_context_window_override_roundtrip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.models = vec![
        crate::provider::ModelConfig {
            name: "m1".to_string(),
            context_window: 128_000,
            ..Default::default()
        },
        crate::provider::ModelConfig {
            name: "m2".to_string(),
            context_window: 800_000,
            ..Default::default()
        },
    ];
    config.agent.default_model = "m1".to_string();
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();

    let new_session = |context_window| crate::kernel::CreateSessionInput {
        project_id: None,
        working_dir: None,
        auto_approve_level: None,
        tool_blocklist: vec![],
        model_key: None,
        context_window,
    };
    let sid = kernel.create_session(new_session(None)).await.unwrap();

    // 无覆盖：生效值 == 模型默认，override 为 None。
    let info = kernel.get_session_context_window(&sid).await.unwrap();
    assert_eq!(info.override_, None);
    assert_eq!(info.effective, info.model_default);
    assert_eq!(info.model_default, 128_000);

    // 设置覆盖：生效值 == 覆盖。
    kernel
        .set_session_context_window(&sid, Some(400_000))
        .await
        .unwrap();
    let info = kernel.get_session_context_window(&sid).await.unwrap();
    assert_eq!(info.override_, Some(400_000));
    assert_eq!(info.effective, 400_000);

    // 换模型不清覆盖：override 保留，model_default 跟随新模型。
    kernel.set_session_model(&sid, "m2").await.unwrap();
    let info = kernel.get_session_context_window(&sid).await.unwrap();
    assert_eq!(info.override_, Some(400_000), "override survives /model");
    assert_eq!(info.effective, 400_000);
    assert_eq!(info.model_default, 800_000);
    assert_eq!(info.model_key, "m2");

    // `Some(0)` 拒绝且不写。
    assert!(kernel
        .set_session_context_window(&sid, Some(0))
        .await
        .is_err());
    let info = kernel.get_session_context_window(&sid).await.unwrap();
    assert_eq!(info.override_, Some(400_000));

    // 清除：回落模型默认。
    kernel.set_session_context_window(&sid, None).await.unwrap();
    let info = kernel.get_session_context_window(&sid).await.unwrap();
    assert_eq!(info.override_, None);
    assert_eq!(info.effective, info.model_default);

    // create 传 Some(0)：过滤为无覆盖。
    let zero_sid = kernel.create_session(new_session(Some(0))).await.unwrap();
    let info = kernel.get_session_context_window(&zero_sid).await.unwrap();
    assert_eq!(info.override_, None, "Some(0) filtered at create");

    // 未知 session：NotFound。
    let missing = crate::types::SessionId::new();
    assert!(kernel
        .set_session_context_window(&missing, Some(1))
        .await
        .is_err());
    assert!(kernel.get_session_context_window(&missing).await.is_err());

    kernel.stop().await;
}

// ── 关停前置：stop_active_runs ─────────────────────────────────────

/// 挂起 mock LLM：SSE 回首个 chunk 后保持连接不再写——agent 停在
/// Streaming，直到 cancel 把流断开。返回监听地址。
async fn hanging_llm_server() -> std::net::SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                // 读完整请求（headers + body），避免客户端早夭。
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let header_end = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
                    {
                        break pos;
                    }
                };
                let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let content_length: usize = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse().ok())
                    })
                    .unwrap_or(0);
                while buf.len() - header_end < content_length {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let body = "data: {\"id\":\"hang\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"stub\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"...\"},\"finish_reason\":null}]}\n\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n{body}"
                );
                if sock.write_all(resp.as_bytes()).await.is_err() {
                    return;
                }
                // 挂起：等客户端断开（cancel 后 agent drop 流），永不写 finish。
                let mut sink = [0u8; 64];
                let _ = sock.read(&mut sink).await;
            });
        }
    });
    addr
}

/// 关停前置：在跑的 run 按 /stop 同路径停完、终态事件投递后 `stop()`
/// 才返回——通道状态卡得以 morph 进终态，而不是冻结在"运行中"。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_winds_down_active_run_before_shutdown() {
    use crate::event::{AgentEvent, AgentStatus, Event, StopReason};
    use crate::provider::ModelConfig;

    let addr = hanging_llm_server().await;

    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.models.clear();
    config.models.push(ModelConfig {
        name: "stub".to_string(),
        model_id: "stub".to_string(),
        endpoint: format!("http://{addr}"),
        api_key: "stub".to_string(),
        context_window: 128_000,
        ..ModelConfig::default()
    });
    config.agent.default_model = "stub".to_string();
    config.finalize();

    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();

    let sid = kernel
        .create_session(super::CreateSessionInput {
            project_id: None,
            working_dir: Some(tmp.path().to_path_buf()),
            auto_approve_level: None,
            tool_blocklist: Vec::new(),
            model_key: None,
            context_window: None,
        })
        .await
        .unwrap();

    // bus 关停后 subscriber 仍能收完存量——收集 task 全程跑，stop() 后汇合。
    let mut events = kernel.event_bus().unwrap().subscribe_all();
    let collect = tokio::spawn(async move {
        let mut saw_shutdown = false;
        while let Some((_, envelope)) = events.recv().await {
            if matches!(
                envelope.event,
                Event::Agent(AgentEvent::Lifecycle {
                    state: AgentStatus::Stopped {
                        reason: StopReason::Shutdown
                    }
                })
            ) {
                saw_shutdown = true;
            }
        }
        saw_shutdown
    });

    kernel
        .send_message_inner(
            &sid,
            vec![crate::types::ContentBlock::Text {
                text: "hi".to_string(),
            }],
            false,
        )
        .await
        .unwrap();

    // mock LLM 挂起 → run 停在 Streaming。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !kernel.conductor.is_running(&sid) {
        assert!(
            std::time::Instant::now() < deadline,
            "run never reached a running state"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let started = std::time::Instant::now();
    kernel.stop().await;
    let elapsed = started.elapsed();

    // stop() 返回时 run 已停完——且远快于 60s 等待上界（cancel 即断流）。
    assert!(!kernel.conductor.is_running(&sid));
    assert!(
        elapsed < std::time::Duration::from_secs(30),
        "wind-down should finish far below the 60s cap, took {elapsed:?}"
    );
    // 终态事件（Stopped{Shutdown}）已在 bus 关停前投递。
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), collect)
            .await
            .expect("event collector hung")
            .expect("event collector panicked"),
        "no Stopped{{Shutdown}} event was delivered before shutdown"
    );
    // 上下文落盘带 shutdown 打断标记（模型下次醒来知道输出被截断）。
    let transcript =
        std::fs::read_to_string(tmp.path().join("sessions").join(format!("{}.jsonl", sid.0)))
            .expect("transcript should exist");
    assert!(
        transcript.contains("[interrupted: daemon shutdown]"),
        "transcript missing the shutdown interruption marker"
    );
}

/// 无在跑 run 时 `stop()` 零开销（不停 run、不留投递 grace）。
#[tokio::test]
async fn stop_without_active_run_returns_immediately() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut config = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    config.gc.auto = false;
    config.finalize();

    let kernel = crate::build_kernel(&config, false).await.unwrap();
    kernel.start();

    let started = std::time::Instant::now();
    kernel.stop().await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "idle shutdown should be near-instant"
    );
}

// ── ext_route（内存回退路径）──────────────────────────────────────────

#[tokio::test]
async fn ext_route_concurrent_same_key_single_winner() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let (r1, r2, r3) = tokio::join!(
        kernel.ext_route("gitlab-ci", "proj1"),
        kernel.ext_route("gitlab-ci", "proj1"),
        kernel.ext_route("gitlab-ci", "proj1"),
    );
    let (s1, c1) = r1.unwrap();
    let (s2, c2) = r2.unwrap();
    let (s3, c3) = r3.unwrap();
    assert_eq!(s1, s2);
    assert_eq!(s2, s3);
    let created = [c1, c2, c3].iter().filter(|c| **c).count();
    assert_eq!(created, 1, "exactly one caller must see created=true");

    // 后续单发调用：复用，不再创建。
    let (s4, c4) = kernel.ext_route("gitlab-ci", "proj1").await.unwrap();
    assert_eq!(s4, s1);
    assert!(!c4);
}
