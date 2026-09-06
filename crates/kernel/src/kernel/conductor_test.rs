use super::{pet_activity, Conductor};
use crate::agent::{AgentConfig, AgentInput, AgentShared};
use crate::comms::{EventBus, InputBus};
use crate::event::{AgentEvent, AgentStatus, Event, InternalEvent, StopReason};
use crate::notification::{AgentActivity, NotificationBus};
use crate::storage::message::jsonl::JsonlMessageStore;
use crate::storage::migrations::run_migrations;
use crate::storage::{MessageStore, NewSession, SessionStore, SqliteSessionStore};
use crate::types::{ContentBlock, Message, Role, SessionId};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[test]
fn pet_activity_only_summarizes_relevant_events() {
    let permission = AgentEvent::PermissionRequest {
        req_id: "request-1".into(),
        session_id: "session-1".into(),
        tool_id: "tool-1".into(),
        tool_name: "shell".into(),
        tool_args: "secret args".to_string(),
        tool_level: "caution".into(),
        reason: "secret reason".into(),
    };

    assert_eq!(
        pet_activity(&permission),
        Some(AgentActivity::PermissionRequested {
            req_id: "request-1".into(),
            target_session_id: "session-1".into(),
        })
    );
    assert_eq!(
        pet_activity(&AgentEvent::Lifecycle {
            state: AgentStatus::Running,
        }),
        Some(AgentActivity::Started)
    );
    assert_eq!(
        pet_activity(&AgentEvent::Retrying {
            attempt: 1,
            max_attempts: 3,
            reason: "not globally forwarded".into(),
            wait_ms: 0,
        }),
        None
    );
}

// ── subagent 无主完成回信转运 ─────────────────────────────────────────
//
// 2026-08-21 post_message follow-up 回信蒸发事故的根治：claim 命中
// （run_subagent 声明）的完成归既有 sync/async 路径；无主完成
// （follow-up、重启恢复）由 conductor 把最终答案转运给 parent。

struct OrphanHarness {
    conductor: Arc<Conductor>,
    input_bus: Arc<InputBus>,
    event_bus: Arc<EventBus>,
    message_store: Arc<dyn MessageStore>,
}

/// 慢写包装：`append` 注入延迟（模拟病态慢盘），其余直传。
struct SlowStore {
    inner: Arc<dyn MessageStore>,
    delay: std::time::Duration,
}

#[async_trait::async_trait]
impl MessageStore for SlowStore {
    async fn append(&self, session_id: &str, messages: &[Message]) -> crate::types::Result<()> {
        tokio::time::sleep(self.delay).await;
        self.inner.append(session_id, messages).await
    }

    async fn get(&self, session_id: &str) -> crate::types::Result<Vec<Message>> {
        self.inner.get(session_id).await
    }

    async fn get_inlined(&self, session_id: &str) -> crate::types::Result<Vec<Message>> {
        self.inner.get_inlined(session_id).await
    }

    async fn replace(&self, session_id: &str, messages: &[Message]) -> crate::types::Result<()> {
        self.inner.replace(session_id, messages).await
    }
}

async fn orphan_harness(sub_id: &SessionId, with_parent: bool) -> OrphanHarness {
    orphan_harness_with(sub_id, with_parent, None).await
}

/// `slow_write`：持久化池的写口包一层慢写（`append` 注入延迟；
/// `get` 直读不减速）——钉死"`MessageAdded` 紧随 `Stopped` 时，
/// 转运必须等最终答案落盘"的集成回归。
async fn orphan_harness_with(
    sub_id: &SessionId,
    with_parent: bool,
    slow_write: Option<std::time::Duration>,
) -> OrphanHarness {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let session_store: Arc<dyn SessionStore> = Arc::new(SqliteSessionStore::new(pool));
    let parent_id = SessionId::from("sess_parent");
    session_store
        .create(NewSession::new(parent_id.clone()))
        .await
        .unwrap();
    session_store
        .create(NewSession {
            parent_id: with_parent.then_some(parent_id),
            ..NewSession::new(sub_id.clone())
        })
        .await
        .unwrap();

    // data_dir 与 jsonl store 都用泄漏的临时目录：run 循环接线测试
    // 会真的 wake_agent，绝不能在仓库 CWD 落测试文件。
    let data_dir = tempfile::TempDir::new().unwrap().keep();
    let store_dir = tempfile::TempDir::new().unwrap().keep();
    let message_store: Arc<dyn MessageStore> =
        Arc::new(JsonlMessageStore::new(&store_dir, &store_dir));

    // 与生产拓扑一致的单一共享事件总线（kernel/mod.rs 同款）。
    let event_bus = EventBus::new();
    let input_bus = InputBus::new();
    let rx = input_bus.subscribe_all();
    let mut shared = AgentShared::new(
        Default::default(),
        String::new(),
        None,
        None,
        None,
        Some(session_store),
        Some(Arc::clone(&message_store)),
        None,
        None,
        Vec::new(),
        None,
        None,
    );
    shared.event_bus = Some(Arc::clone(&event_bus));
    // 与生产拓扑一致的持久化池（run 循环的 MessageAdded/Stopped
    // 臂都走它；旧 inline append 已退役）。
    let write_store: Arc<dyn MessageStore> = match slow_write {
        Some(delay) => Arc::new(SlowStore {
            inner: Arc::clone(&message_store),
            delay,
        }),
        None => Arc::clone(&message_store),
    };
    shared.persist_pool = Some(Arc::new(crate::kernel::persist_pool::build(
        write_store,
        CancellationToken::new(),
    )));
    let conductor = Arc::new(Conductor::new(
        Arc::new(shared),
        AgentConfig::default(),
        rx,
        Arc::clone(&event_bus),
        Arc::clone(&input_bus),
        String::new(),
        data_dir,
        Arc::new(NotificationBus::new()),
    ));
    OrphanHarness {
        conductor,
        input_bus,
        event_bus,
        message_store,
    }
}

async fn put_reply(store: &Arc<dyn MessageStore>, sid: &SessionId, text: &str) {
    store
        .append(
            &sid.0,
            &[Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
                ..Default::default()
            }],
        )
        .await
        .unwrap();
}

fn completed() -> StopReason {
    StopReason::Completed {
        finish_reason: None,
    }
}

/// claim 消费语义：声明过的完成被跳过（且 claim 一次性消耗——同一
/// session 的下一次完成视为无主）。
#[tokio::test]
async fn orphan_detection_consumes_claim_once() {
    let sub = SessionId::from("sub_claimed");
    let h = orphan_harness(&sub, true).await;

    h.conductor.agent_shared.subagent_claims.insert(sub.clone());
    assert!(
        !h.conductor.should_forward_orphan(&sub),
        "claimed completion belongs to run_subagent's own paths"
    );
    assert!(
        h.conductor.should_forward_orphan(&sub),
        "claim consumed — next completion is an orphan"
    );
    // 非 subagent 会话永不转运。
    let plain = SessionId::from("sess_plain");
    assert!(!h.conductor.should_forward_orphan(&plain));
}

/// 无主完成（Completed）：最终答案以 [From Agent: ...] 格式转运给 parent。
#[tokio::test]
async fn orphan_completion_forwards_reply_to_parent() {
    let sub = SessionId::from("sub_orphan");
    let h = orphan_harness(&sub, true).await;
    put_reply(&h.message_store, &sub, "复审结论：APPROVE").await;
    let mut parent_rx = h.input_bus.subscribe(SessionId::from("sess_parent"));

    assert!(h.conductor.should_forward_orphan(&sub));
    h.conductor.forward_orphan_reply(&sub, &completed()).await;

    let (sid, input) = tokio::time::timeout(std::time::Duration::from_secs(1), parent_rx.recv())
        .await
        .expect("reply was not forwarded to parent")
        .expect("input bus closed");
    assert_eq!(sid, SessionId::from("sess_parent"));
    let AgentInput::Steer(content) = input else {
        panic!("expected Steer input");
    };
    assert_eq!(
        content,
        vec![ContentBlock::Text {
            text: "[From Agent: sub_orphan] Follow-up reply\n复审结论：APPROVE".to_string(),
        }]
    );
}

/// Cancelled 不转运（多半是 parent 自己 /stop 的）。
#[tokio::test]
async fn cancelled_orphan_is_not_forwarded() {
    let sub = SessionId::from("sub_cancelled");
    let h = orphan_harness(&sub, true).await;
    put_reply(&h.message_store, &sub, "半截答案").await;
    let mut parent_rx = h.input_bus.subscribe(SessionId::from("sess_parent"));

    h.conductor
        .forward_orphan_reply(
            &sub,
            &StopReason::Cancelled {
                operation: Some("streaming".to_string()),
            },
        )
        .await;

    let got = tokio::time::timeout(std::time::Duration::from_millis(200), parent_rx.recv()).await;
    assert!(got.is_err(), "cancelled run must not be forwarded");
}

/// Failed 完成带 ⚠ 标注转运（残留文本不静默丢弃）。
#[tokio::test]
async fn failed_orphan_forwards_with_warning_tag() {
    let sub = SessionId::from("sub_failed");
    let h = orphan_harness(&sub, true).await;
    put_reply(&h.message_store, &sub, "半截答案").await;
    let mut parent_rx = h.input_bus.subscribe(SessionId::from("sess_parent"));

    h.conductor
        .forward_orphan_reply(
            &sub,
            &StopReason::Failed {
                error: "model blew up".to_string(),
            },
        )
        .await;

    let (_sid, input) = tokio::time::timeout(std::time::Duration::from_secs(1), parent_rx.recv())
        .await
        .expect("failed run's partial reply was not forwarded")
        .expect("input bus closed");
    let AgentInput::Steer(content) = input else {
        panic!("expected Steer input");
    };
    let text = match &content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text block"),
    };
    assert!(
        text.starts_with(
            "[From Agent: sub_failed] Follow-up reply\n⚠ run failed: model blew up\n半截答案"
        ),
        "unexpected forwarded text: {text}"
    );
}

/// 无 parent 的无主完成：warn 丢弃，不转运（无处可去）。
#[tokio::test]
async fn orphan_without_parent_is_dropped() {
    let sub = SessionId::from("sub_parentless");
    let h = orphan_harness(&sub, false).await;
    put_reply(&h.message_store, &sub, "无家可归的答案").await;
    let mut all_rx = h.input_bus.subscribe_all();

    h.conductor.forward_orphan_reply(&sub, &completed()).await;

    let got = tokio::time::timeout(std::time::Duration::from_millis(200), all_rx.recv()).await;
    assert!(
        got.is_err(),
        "parentless orphan reply must not be forwarded"
    );
}

/// 无文本答案（纯 tool_calls 中间轮结尾等病态边缘）：不转运。
#[tokio::test]
async fn orphan_without_text_reply_is_dropped() {
    let sub = SessionId::from("sub_notext");
    let h = orphan_harness(&sub, true).await;
    h.message_store
        .append(
            &sub.0,
            &[Message {
                role: Role::Assistant,
                content: vec![],
                tool_calls: Some(vec![crate::types::ToolCall {
                    id: "tc1".to_string(),
                    name: "read".to_string(),
                    arguments: serde_json::json!({}),
                }]),
                ..Default::default()
            }],
        )
        .await
        .unwrap();
    let mut all_rx = h.input_bus.subscribe_all();

    h.conductor.forward_orphan_reply(&sub, &completed()).await;

    let got = tokio::time::timeout(std::time::Duration::from_millis(200), all_rx.recv()).await;
    assert!(got.is_err(), "empty reply must not be forwarded");
}

/// run 循环全接线（复审 should-fix）：事件总线进 MessageAdded
/// （持久化池 dispatch + `Stopped` 臂 wait_idle 排空）→ Stopped
/// （消费 claim、spawn 转运）→ parent 收到 steer。钉住的是分发臂
/// 接线本身，不是直调 helper。
#[tokio::test]
async fn run_loop_forwards_orphan_stop_to_parent() {
    let sub = SessionId::from("sub_wired");
    let h = orphan_harness(&sub, true).await;
    let mut parent_rx = h.input_bus.subscribe(SessionId::from("sess_parent"));
    let token = CancellationToken::new();
    let run = {
        let c = Arc::clone(&h.conductor);
        let token = token.clone();
        tokio::spawn(async move { c.run(token).await })
    };

    // 等 run() 完成事件总线订阅（subscribe_all 在 run 开头；直接发
    // 布会抢在 listener 注册前，事件被静默丢弃）。
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let msg = Arc::new(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "接线版答案".to_string(),
        }],
        ..Default::default()
    });
    h.event_bus
        .publish(
            sub.clone(),
            crate::event::Envelope::new(
                sub.clone(),
                Event::Internal(InternalEvent::MessageAdded { message: msg }),
            ),
        )
        .unwrap();
    h.event_bus
        .publish(
            sub.clone(),
            crate::event::Envelope::new(
                sub.clone(),
                Event::Agent(AgentEvent::Lifecycle {
                    state: AgentStatus::Stopped {
                        reason: completed(),
                    },
                }),
            ),
        )
        .unwrap();

    let (_sid, input) = tokio::time::timeout(std::time::Duration::from_secs(3), parent_rx.recv())
        .await
        .expect("orphan stop was not forwarded through the run loop")
        .expect("input bus closed");
    let AgentInput::Steer(content) = input else {
        panic!("expected Steer input");
    };
    assert_eq!(
        content,
        vec![ContentBlock::Text {
            text: "[From Agent: sub_wired] Follow-up reply\n接线版答案".to_string(),
        }]
    );

    token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), run).await;
}

/// `Stopped` 臂 `wait_idle` 的集成回归（复审 should-fix）：慢写盘
/// + `MessageAdded` 紧随 `Stopped`——转运必须等到最终答案落盘。
/// （没有 `wait_idle` 时 80ms 慢写下转运读空答案、parent 收不到
/// steer，本测试即红。）
#[tokio::test]
async fn stopped_waits_for_slow_persist_before_forwarding() {
    let sub = SessionId::from("sub_slow_persist");
    let h = orphan_harness_with(&sub, true, Some(std::time::Duration::from_millis(80))).await;
    let mut parent_rx = h.input_bus.subscribe(SessionId::from("sess_parent"));
    let token = CancellationToken::new();
    let run = {
        let c = Arc::clone(&h.conductor);
        let token = token.clone();
        tokio::spawn(async move { c.run(token).await })
    };

    // 等 run() 完成事件总线订阅（同 run_loop 接线测试）。
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let msg = Arc::new(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "慢盘最终答案".to_string(),
        }],
        ..Default::default()
    });
    h.event_bus
        .publish(
            sub.clone(),
            crate::event::Envelope::new(
                sub.clone(),
                Event::Internal(InternalEvent::MessageAdded { message: msg }),
            ),
        )
        .unwrap();
    h.event_bus
        .publish(
            sub.clone(),
            crate::event::Envelope::new(
                sub.clone(),
                Event::Agent(AgentEvent::Lifecycle {
                    state: AgentStatus::Stopped {
                        reason: completed(),
                    },
                }),
            ),
        )
        .unwrap();

    let (_sid, input) = tokio::time::timeout(std::time::Duration::from_secs(3), parent_rx.recv())
        .await
        .expect("orphan stop was not forwarded under slow persist")
        .expect("input bus closed");
    let AgentInput::Steer(content) = input else {
        panic!("expected Steer input");
    };
    assert_eq!(
        content,
        vec![ContentBlock::Text {
            text: "[From Agent: sub_slow_persist] Follow-up reply\n慢盘最终答案".to_string(),
        }]
    );

    token.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), run).await;
}

/// MaxIterations 完成带 ⚠ 标注转运。
#[tokio::test]
async fn max_iterations_orphan_forwards_with_warning_tag() {
    let sub = SessionId::from("sub_maxiter");
    let h = orphan_harness(&sub, true).await;
    put_reply(&h.message_store, &sub, "写到一半的答案").await;
    let mut parent_rx = h.input_bus.subscribe(SessionId::from("sess_parent"));

    h.conductor
        .forward_orphan_reply(&sub, &StopReason::MaxIterations { reached: 12 })
        .await;

    let (_sid, input) = tokio::time::timeout(std::time::Duration::from_secs(1), parent_rx.recv())
        .await
        .expect("max-iterations reply was not forwarded")
        .expect("input bus closed");
    let AgentInput::Steer(content) = input else {
        panic!("expected Steer input");
    };
    let text = match &content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => panic!("expected text block"),
    };
    assert!(
        text.starts_with(
            "[From Agent: sub_maxiter] Follow-up reply\n⚠ run hit max iterations (12)\n写到一半的答案"
        ),
        "unexpected forwarded text: {text}"
    );
}
