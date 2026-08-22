use super::*;
use crate::channels::hub::ChannelInstance;
use crate::channels::store::SqliteChannelStore;
use crate::channels::{ChannelConfig, ChannelError, ChannelEvent, PlatformAdapter};
use crate::event::StopReason;
use crate::storage::migrations::run_migrations;
use crate::types::ContentBlock;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::sync::mpsc as tokio_mpsc;

/// 极简 adapter：只记录 `send_message` 的文本（typing 用默认 no-op）。
struct MockAdapter {
    sent: tokio::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: tokio_mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        _chat: &str,
        blocks: Vec<ContentBlock>,
        _anchor: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        let text = blocks
            .iter()
            .find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        self.sent.lock().await.push(text);
        Ok(Some("m-1".into()))
    }
}

fn test_routing() -> Arc<SessionRouting> {
    Arc::new(SessionRouting {
        channel_name: "feishu".to_string(),
        external_chat_id: "chat-1".to_string(),
        reply_msg_id: None,
        mapping_key: "chat-1".to_string(),
        doc_comment: None,
    })
}

/// 端到端：Running → End(文本) → Stopped 流经 actor 后，回复必须
/// 经 adapter 送出（2026-08-21 事故的正面回归：投递链路工作）。
#[tokio::test]
async fn actor_delivers_reply_on_stopped() {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&db).await.unwrap();
    let store: Arc<dyn ChannelStore> = Arc::new(SqliteChannelStore::new(db));
    let sid = SessionId::from("sess_actor_e2e");
    store
        .save_mapping("feishu", "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();

    let adapter = Arc::new(MockAdapter {
        sent: tokio::sync::Mutex::new(Vec::new()),
    });
    let mut config = ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        ..ChannelConfig::default()
    };
    // 纯文本投递路径（无状态卡分支，直接断言 send_message）。
    config.observability = false;
    let instances: Arc<DashMap<String, ChannelInstance>> = Arc::new(DashMap::new());
    instances.insert(
        "feishu".to_string(),
        ChannelInstance::test_instance(config, adapter.clone()),
    );

    let pool = DeliveryPool::new(
        Arc::new(ObsTracker::new()),
        Arc::new(AskCardRegistry::new()),
        store,
        instances,
        std::sync::Weak::new(),
        CancellationToken::new(),
    );

    let routing = test_routing();
    let events = vec![
        Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Running,
        }),
        Event::Model(ModelEvent::End {
            message_id: "m-1".into(),
            content: vec![ContentBlock::Text {
                text: "答案42".to_string(),
            }],
        }),
        Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Stopped {
                reason: StopReason::Completed {
                    finish_reason: None,
                },
            },
        }),
    ];
    for event in events {
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event,
            },
        );
    }

    // actor 是异步的：轮询直到投递落地（超时即失败）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if adapter
            .sent
            .lock()
            .await
            .iter()
            .any(|t| t.contains("答案42"))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "reply was not delivered by the session actor"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// 事故核心回归（评审 should-fix #4）：`Stopped` 被 bus 丢弃（只发
/// Running + End，永远不发 `Stopped`）→ actor 的巡检判死（注入
/// `agent_dead=true`）后必须把残余回复以 Timeout 形态兜底送出。
#[tokio::test]
async fn actor_settles_reply_when_stopped_lost() {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&db).await.unwrap();
    let store: Arc<dyn ChannelStore> = Arc::new(SqliteChannelStore::new(db));
    let sid = SessionId::from("sess_actor_settle");
    store
        .save_mapping("feishu", "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();

    let adapter = Arc::new(MockAdapter {
        sent: tokio::sync::Mutex::new(Vec::new()),
    });
    let mut config = ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        ..ChannelConfig::default()
    };
    config.observability = false;
    let instances: Arc<DashMap<String, ChannelInstance>> = Arc::new(DashMap::new());
    instances.insert(
        "feishu".to_string(),
        ChannelInstance::test_instance(config, adapter.clone()),
    );

    // 判死探针恒真（模拟 agent 已死），巡检节拍 50ms。
    let pool = DeliveryPool::for_test(store, instances);

    let routing = test_routing();
    // 只发 Running + End——Stopped“丢了”。
    for event in [
        Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Running,
        }),
        Event::Model(ModelEvent::End {
            message_id: "m-1".into(),
            content: vec![ContentBlock::Text {
                text: "兜底答案7".to_string(),
            }],
        }),
    ] {
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event,
            },
        );
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if adapter
            .sent
            .lock()
            .await
            .iter()
            .any(|t| t.contains("兜底答案7"))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lost-Stopped reply was not settled via the actor watchdog"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// 旗标绑反回归（发版终审 BLOCK 项）：`observability=false` +
/// `tool_trace=true` 的配置下，`channel_flags` 必须按名字映射——
/// 历史上此处按位置传递布尔导致两旗标互换（同 true 时无症状，
/// 配置不同才爆炸）。
#[tokio::test]
async fn channel_flags_maps_config_by_name() {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&db).await.unwrap();
    let store: Arc<dyn ChannelStore> = Arc::new(SqliteChannelStore::new(db));

    let adapter = Arc::new(MockAdapter {
        sent: tokio::sync::Mutex::new(Vec::new()),
    });
    let mut config = ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        ..ChannelConfig::default()
    };
    config.observability = false;
    config.tool_trace = true;
    let instances: Arc<DashMap<String, ChannelInstance>> = Arc::new(DashMap::new());
    instances.insert(
        "feishu".to_string(),
        ChannelInstance::test_instance(config, adapter),
    );

    let pool = DeliveryPool::for_test(store, instances);
    let flags = channel_flags(&test_routing(), &pool.ctx).expect("instance exists");
    assert!(!flags.observability, "observability must map from config");
    assert!(flags.tool_trace, "tool_trace must map from config");
}

/// 新测试的搭建辅助：内存 store + 单 feishu 实例（纯文本投递），
/// 判死探针/节拍/TTL/取消令牌可注入。返回 (pool, adapter, sid)。
async fn setup_pool(
    agent_dead: Box<dyn Fn(&SessionId) -> bool + Send + Sync>,
    settle_interval: std::time::Duration,
    idle_ttl: std::time::Duration,
    token: CancellationToken,
) -> (DeliveryPool, Arc<MockAdapter>, SessionId) {
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&db).await.unwrap();
    let store: Arc<dyn ChannelStore> = Arc::new(SqliteChannelStore::new(db));
    let sid = SessionId::from("sess_pool_test");
    store
        .save_mapping("feishu", "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();

    let adapter = Arc::new(MockAdapter {
        sent: tokio::sync::Mutex::new(Vec::new()),
    });
    let mut config = ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        ..ChannelConfig::default()
    };
    config.observability = false;
    let instances: Arc<DashMap<String, ChannelInstance>> = Arc::new(DashMap::new());
    instances.insert(
        "feishu".to_string(),
        ChannelInstance::test_instance(config, adapter.clone()),
    );

    let pool = DeliveryPool::with_timing(
        Arc::new(ObsTracker::new()),
        Arc::new(AskCardRegistry::new()),
        store,
        instances,
        std::sync::Weak::new(),
        agent_dead,
        token,
        settle_interval,
        idle_ttl,
    );
    (pool, adapter, sid)
}

fn run_events(text: &str) -> Vec<Event> {
    vec![
        Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Running,
        }),
        Event::Model(ModelEvent::End {
            message_id: "m-1".into(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }),
        Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Stopped {
                reason: StopReason::Completed {
                    finish_reason: None,
                },
            },
        }),
    ]
}

async fn wait_delivered(adapter: &MockAdapter, text: &str, what: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if adapter.sent.lock().await.iter().any(|t| t.contains(text)) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// 过期设计回归（2026-08-22 定稿）：闲置超 TTL 的 worker 自我过
/// 期（entry 摘除，真回收）；后续 dispatch 惰性重建，正常投递。
#[tokio::test]
async fn idle_worker_expires_and_respawns_on_demand() {
    let (pool, adapter, sid) = setup_pool(
        Box::new(|_| false),
        std::time::Duration::from_millis(20),
        std::time::Duration::from_millis(60),
        CancellationToken::new(),
    )
    .await;
    let routing = test_routing();

    for event in run_events("第一轮回复") {
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event,
            },
        );
    }
    wait_delivered(&adapter, "第一轮回复", "first run not delivered").await;

    // 越过 TTL+数个节拍：worker 已自我过期，entry 被摘除。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while pool.actors.contains_key(&sid) {
        assert!(
            std::time::Instant::now() < deadline,
            "idle worker was not expired"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(pool.is_quiet(&sid), "expired session reads quiet");

    // 第二轮：dispatch 惰性重建 worker 并正常投递。
    for event in run_events("第二轮回复") {
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event,
            },
        );
    }
    wait_delivered(&adapter, "第二轮回复", "post-expiry run not delivered").await;
}

/// 防劈 run 第一防线（整审 should-fix）：buffer 在飞 ⟹ 永不过期
/// ——`else if` 顺序一旦破坏，mid-run worker 过期、新 worker 无
/// buffer、`Stopped` 结不到回复。钉死：持有 buffer 越过 TTL 仍
/// 存活，buffer 结算后才允许过期。
#[tokio::test]
async fn in_flight_buffer_blocks_expiry() {
    let (pool, adapter, sid) = setup_pool(
        Box::new(|_| false),
        std::time::Duration::from_millis(20),
        std::time::Duration::from_millis(60),
        CancellationToken::new(),
    )
    .await;
    let routing = test_routing();

    // Running 建 buffer（无 Stopped）——越过 TTL 数个节拍：不得过期。
    pool.dispatch(
        &sid,
        DeliveryJob {
            routing: Arc::clone(&routing),
            event: Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }),
        },
    );
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        pool.actors.contains_key(&sid),
        "worker expired while a run was in flight"
    );

    // 结算（Stopped）后 buffer 清空：越过 TTL 必须过期。
    for event in run_events("收尾").into_iter().skip(1) {
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event,
            },
        );
    }
    wait_delivered(&adapter, "收尾", "settle not delivered").await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while pool.actors.contains_key(&sid) {
        assert!(
            std::time::Instant::now() < deadline,
            "worker not expired after buffer settled"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Full 丢件分支（整审 should-fix，v0.9.6 前既有缺口）：队列打满
/// 时 dispatch 丢件记 ERROR 且不 panic、不影响后续投递。
#[tokio::test]
async fn full_queue_drops_event_without_harm() {
    let (pool, adapter, sid) = setup_pool(
        Box::new(|_| false),
        std::time::Duration::from_hours(1),
        std::time::Duration::from_hours(1),
        CancellationToken::new(),
    )
    .await;
    let routing = test_routing();

    // 植入 worker 永不消费（pending）且通道已满的 entry。
    let (tx, rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
    for _ in 0..SESSION_EVENT_CAPACITY {
        tx.try_send(DeliveryJob {
            routing: Arc::clone(&routing),
            event: Event::Model(ModelEvent::Request {
                message_id: crate::types::MessageId::new(),
                message_count: 1,
            }),
        })
        .unwrap();
    }
    pool.actors.insert(
        sid.clone(),
        ActorHandle {
            tx,
            worker: tokio::spawn(async move {
                let _rx = rx;
                std::future::pending::<()>().await;
            }),
            last_activity: std::time::Instant::now(),
            inflight: Arc::new(AtomicU32::new(0)),
            has_buffer: Arc::new(AtomicBool::new(false)),
        },
    );

    // 打满后再投：丢件、不 panic。
    pool.dispatch(
        &sid,
        DeliveryJob {
            routing: Arc::clone(&routing),
            event: Event::Model(ModelEvent::Request {
                message_id: crate::types::MessageId::new(),
                message_count: 1,
            }),
        },
    );
    // entry 未被破坏（worker 仍在）。
    assert!(pool.actors.contains_key(&sid));
    assert!(
        adapter.sent.lock().await.is_empty(),
        "dropped event must not be delivered"
    );
}

/// TTL 由事件到达驱动：判定节拍之间有事件到达（即便不产生
/// buffer 的事件），worker 不得过期——entry 时间戳在锁内被刷
/// 新，过期复核必然放弃。
#[tokio::test]
async fn event_arrival_defers_expiry() {
    let (pool, adapter, sid) = setup_pool(
        Box::new(|_| false),
        std::time::Duration::from_millis(20),
        std::time::Duration::from_millis(120),
        CancellationToken::new(),
    )
    .await;
    let routing = test_routing();

    // 每 50ms 来一条不产生 buffer 的事件（ModelEvent::Request），
    // 总时长 300ms 远超 TTL=120ms——worker 必须始终存活。
    for _ in 0..6 {
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event: Event::Model(ModelEvent::Request {
                    message_id: crate::types::MessageId::new(),
                    message_count: 1,
                }),
            },
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        pool.actors.contains_key(&sid),
        "worker expired despite fresh event arrivals"
    );

    // 静默超过 TTL：必须过期。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while pool.actors.contains_key(&sid) {
        assert!(
            std::time::Instant::now() < deadline,
            "worker not expired after silence"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let _ = adapter;
}

/// Closed 分支（worker 猝死的保险丝）：entry 残留死通道时，
/// dispatch 原地换代并重投——投递不受影响。
#[tokio::test]
async fn closed_branch_respawns_after_abnormal_death() {
    let (pool, adapter, sid) = setup_pool(
        Box::new(|_| false),
        std::time::Duration::from_hours(1),
        std::time::Duration::from_hours(1),
        CancellationToken::new(),
    )
    .await;
    let routing = test_routing();

    // 植入尸体：tx 存活但 rx 已弃（channel 立闭），worker 句柄已完结。
    let (tx, rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
    drop(rx);
    pool.actors.insert(
        sid.clone(),
        ActorHandle {
            tx,
            worker: tokio::spawn(async {}),
            last_activity: std::time::Instant::now(),
            inflight: Arc::new(AtomicU32::new(0)),
            has_buffer: Arc::new(AtomicBool::new(false)),
        },
    );

    for event in run_events("换代后投递") {
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event,
            },
        );
    }
    wait_delivered(&adapter, "换代后投递", "run not delivered after respawn").await;
    // 换代后 entry 的时间戳必须是新的（spawn_handle 创建即写入），
    // 新 worker 不会因陈旧时间戳立刻过期（复审 should-fix pin）。
    let fresh = pool
        .actors
        .get(&sid)
        .is_some_and(|h| h.last_activity.elapsed() < std::time::Duration::from_secs(1));
    assert!(fresh, "respawned worker has a stale last_activity");
    // janitor 不得误收活 worker。
    pool.janitor_sweep();
    assert!(pool.actors.contains_key(&sid));
}

/// janitor 收尸：worker 已完结的 entry（含脏旗标）被摘除；活
/// worker 不动。
#[tokio::test]
async fn janitor_collects_corpses_keeps_living() {
    let (pool, _adapter, sid) = setup_pool(
        Box::new(|_| false),
        std::time::Duration::from_hours(1),
        std::time::Duration::from_hours(1),
        CancellationToken::new(),
    )
    .await;

    // 活 worker（正常 dispatch 建）。
    pool.dispatch(
        &sid,
        DeliveryJob {
            routing: test_routing(),
            event: Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }),
        },
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !pool.actors.contains_key(&sid) {
        assert!(
            std::time::Instant::now() < deadline,
            "worker never registered"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // 尸体：另一个 session，worker 已完结且旗标脏。
    let corpse = SessionId::from("sess_corpse");
    // 植入尸体：tx 存活但 rx 已弃（channel 立闭），worker 句柄已完结。
    let (tx, rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
    drop(rx);
    let corpse_worker = tokio::spawn(async {});
    while !corpse_worker.is_finished() {
        tokio::task::yield_now().await;
    }
    pool.actors.insert(
        corpse.clone(),
        ActorHandle {
            tx,
            worker: corpse_worker,
            last_activity: std::time::Instant::now(),
            inflight: Arc::new(AtomicU32::new(2)),
            has_buffer: Arc::new(AtomicBool::new(true)),
        },
    );

    pool.janitor_sweep();
    assert!(
        !pool.actors.contains_key(&corpse),
        "corpse entry must be collected"
    );
    assert!(
        pool.actors.contains_key(&sid),
        "living worker must survive the janitor"
    );
}

/// 三审 should-fix #2 回归：判死探针 panic 必须降级为"视为存活"
/// ——actor 不死、buffer 不被误结算，后续事件正常投递。
#[tokio::test]
async fn panicking_probe_is_downgraded_to_alive() {
    let probe_calls = Arc::new(AtomicU32::new(0));
    let probe = {
        let calls = Arc::clone(&probe_calls);
        move |_: &SessionId| -> bool {
            calls.fetch_add(1, Ordering::Relaxed);
            panic!("injected probe panic")
        }
    };
    let (pool, adapter, sid) = setup_pool(
        Box::new(probe),
        std::time::Duration::from_millis(30),
        std::time::Duration::from_hours(1),
        CancellationToken::new(),
    )
    .await;
    let routing = test_routing();

    // ① Running 建 buffer；② 等巡检节拍触发探针（panic → 降级
    // 存活）；③ 补齐 End+Stopped：actor 必须活着并正常投递。
    pool.dispatch(
        &sid,
        DeliveryJob {
            routing: Arc::clone(&routing),
            event: Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }),
        },
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while probe_calls.load(Ordering::Relaxed) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "watchdog tick never invoked the probe"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    for event in run_events("探针panic后仍投递").into_iter().skip(1) {
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event,
            },
        );
    }
    wait_delivered(&adapter, "探针panic后仍投递", "run lost after probe panic").await;
    assert!(
        pool.actors.contains_key(&sid),
        "actor must survive probe panics"
    );
}

/// 关停路径回归：token 取消后 worker 退出（句柄完结），尸体由
/// janitor 收殓；其后的 dispatch 重建登记（此 pool 的 token 已
/// 死，新 worker 即生即灭属预期——此处只钉"收尸→重建"链路）。
#[tokio::test]
async fn cancelled_worker_is_collected_and_respawned() {
    let token = CancellationToken::new();
    let (pool, _adapter, sid) = setup_pool(
        Box::new(|_| false),
        std::time::Duration::from_hours(1),
        std::time::Duration::from_hours(1),
        token.clone(),
    )
    .await;

    pool.dispatch(
        &sid,
        DeliveryJob {
            routing: test_routing(),
            event: Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }),
        },
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !pool.actors.contains_key(&sid) {
        assert!(
            std::time::Instant::now() < deadline,
            "worker never registered"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    token.cancel();

    // worker 退出（句柄完结）；entry 尚在（尸体），janitor 收殓。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let finished = pool.actors.get(&sid).is_none_or(|h| h.worker.is_finished());
        if finished {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not exit on cancel"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    pool.janitor_sweep();
    assert!(
        !pool.actors.contains_key(&sid),
        "janitor must collect the cancelled worker's corpse"
    );

    // 后续 dispatch 经 Vacant 路径重建登记。
    pool.dispatch(
        &sid,
        DeliveryJob {
            routing: test_routing(),
            event: Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }),
        },
    );
    assert!(
        pool.actors.contains_key(&sid),
        "dispatch must respawn after corpse collection"
    );
}

/// S2 回归：并发会话数超过 IO 信号量（16）时全量投递仍完成——
/// settle 的 permit 单点获取若退化为嵌套获取，此用例会死锁超时。
#[tokio::test]
async fn concurrent_sessions_all_deliver_under_io_cap() {
    const SESSIONS: usize = 20;
    let (pool, adapter, _sid) = setup_pool(
        Box::new(|_| false),
        std::time::Duration::from_millis(50),
        std::time::Duration::from_hours(1),
        CancellationToken::new(),
    )
    .await;
    let routing = test_routing();

    for i in 0..SESSIONS {
        // 每个会话独立的 mapping 键（同键 upsert 会互相覆盖，
        // settle 新鲜重读路由时前 19 个会话会查无路由）。
        let sid = SessionId::from(format!("sess_concurrent_{i}"));
        pool.ctx
            .store
            .save_mapping("feishu", &format!("chat-{i}"), &sid, "chat-1", None)
            .await
            .unwrap();
        for event in run_events(&format!("并发回复{i}")) {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event,
                },
            );
        }
    }
    for i in 0..SESSIONS {
        wait_delivered(&adapter, &format!("并发回复{i}"), "concurrent run lost").await;
    }
}
