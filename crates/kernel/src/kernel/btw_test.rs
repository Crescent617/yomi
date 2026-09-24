use super::*;

fn text_msg(role: Role, text: &str) -> Arc<Message> {
    Arc::new(Message::with_blocks(
        role,
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    ))
}

fn roles_of(messages: &[Arc<Message>]) -> Vec<Role> {
    messages.iter().map(|m| m.role).collect()
}

#[test]
fn wrapper_forbids_tools_and_carries_question_slot() {
    // 包裹指令的三个硬要求：明令禁调工具（且说透理由）、限制篇幅、
    // 留出"这需要正式提问"的降级话术；末尾以「问题：」收口供拼接。
    assert!(BTW_WRAPPER.contains("不要调用任何工具"));
    assert!(BTW_WRAPPER.contains("这需要正式提问"));
    assert!(BTW_WRAPPER.ends_with("问题："));
}

#[test]
fn snapshot_appends_wrapper_as_trailing_user_message() {
    let history = vec![text_msg(Role::System, "sys"), text_msg(Role::User, "hi")];
    let snap = build_btw_snapshot(&history, None, "问啥");
    assert_eq!(roles_of(&snap), [Role::System, Role::User, Role::User]);
    let body = snap.last().unwrap().content[0].as_text().unwrap();
    assert!(body.starts_with(BTW_WRAPPER));
    assert!(body.ends_with("问啥"));
}

#[test]
fn snapshot_merges_partial_into_trailing_assistant() {
    // /continue 触发的 turn 尾部是 assistant：新起一条会产生相邻同角色
    // （Anthropic 400）——必须并入尾部。
    let history = vec![
        text_msg(Role::System, "sys"),
        text_msg(Role::User, "q"),
        text_msg(Role::Assistant, "a1"),
    ];
    let snap = build_btw_snapshot(&history, Some("半截".to_string()), "问");
    let roles = roles_of(&snap);
    assert_eq!(
        roles,
        [Role::System, Role::User, Role::Assistant, Role::User]
    );
    let texts: Vec<_> = snap[2].content.iter().filter_map(|b| b.as_text()).collect();
    assert_eq!(texts, ["a1", "半截"]);
}

#[test]
fn snapshot_pushes_partial_as_new_message_after_user_tail() {
    let history = vec![text_msg(Role::System, "sys"), text_msg(Role::User, "q")];
    let snap = build_btw_snapshot(&history, Some("半截".to_string()), "问");
    assert_eq!(
        roles_of(&snap),
        [Role::System, Role::User, Role::Assistant, Role::User]
    );
}

#[test]
fn snapshot_drops_dangling_tool_batch() {
    let mut pending = Message::with_blocks(Role::Assistant, vec![]);
    pending.tool_calls = Some(vec![crate::types::ToolCall {
        id: "c1".to_string(),
        name: "shell".to_string(),
        arguments: serde_json::json!({}),
    }]);
    let history = vec![
        text_msg(Role::System, "sys"),
        text_msg(Role::User, "q"),
        Arc::new(pending),
    ];
    let snap = build_btw_snapshot(&history, None, "问");
    assert!(!snap.iter().any(|m| m.tool_calls.is_some()));
    assert_eq!(roles_of(&snap), [Role::System, Role::User, Role::User]);
}

// ── BtwTracker：事件流半截镜像与句柄生命周期 ──────────────────────

fn sid() -> SessionId {
    SessionId::from("sess_test".to_string())
}

fn chunk(message_id: &str, text: &str) -> Event {
    Event::Model(ModelEvent::Chunk {
        message_id: MessageId::from(message_id.to_string()),
        content: ContentChunk::Text(text.to_string()),
    })
}

fn tracker() -> BtwTracker {
    BtwTracker::new(crate::comms::EventBus::new())
}

#[tokio::test]
async fn partial_accumulates_chunks_and_resets_per_message() {
    let tracker = tracker();
    let sid = sid();
    tracker.observe(&sid, &chunk("msg_1", "你"));
    tracker.observe(&sid, &chunk("msg_1", "好"));
    assert_eq!(tracker.partial_of(&sid).as_deref(), Some("你好"));

    // 新 message 的第一个 chunk 重置条目（同 session 单流，不会交错）。
    tracker.observe(&sid, &chunk("msg_2", "新"));
    assert_eq!(tracker.partial_of(&sid).as_deref(), Some("新"));
}

#[tokio::test]
async fn partial_cleared_when_assistant_message_lands() {
    let tracker = tracker();
    let sid = sid();
    tracker.observe(&sid, &chunk("msg_1", "半截"));
    // 同一 message_id 的完整 assistant 落盘 → 半截清除（去重窗口关闭）。
    let mut landed = Message::assistant("完整");
    landed.id = MessageId::from("msg_1".to_string());
    tracker.observe(
        &sid,
        &Event::Internal(InternalEvent::MessageAdded {
            message: Arc::new(landed),
        }),
    );
    assert_eq!(tracker.partial_of(&sid), None);
}

#[tokio::test]
async fn partial_not_cleared_by_unrelated_message() {
    let tracker = tracker();
    let sid = sid();
    tracker.observe(&sid, &chunk("msg_1", "半截"));
    // user 消息落盘不清；别的 message_id 的 assistant 也不清。
    tracker.observe(
        &sid,
        &Event::Internal(InternalEvent::MessageAdded {
            message: Arc::new(Message::user("问")),
        }),
    );
    tracker.observe(
        &sid,
        &Event::Internal(InternalEvent::MessageAdded {
            message: Arc::new(Message::assistant("别的")),
        }),
    );
    assert_eq!(tracker.partial_of(&sid).as_deref(), Some("半截"));
}

#[tokio::test]
async fn partial_cleared_on_idle() {
    let tracker = tracker();
    let sid = sid();
    tracker.observe(&sid, &chunk("msg_1", "被掐断的"));
    tracker.observe(
        &sid,
        &Event::Agent(AgentEvent::StateChanged {
            state: AgentState::Idle,
        }),
    );
    assert_eq!(tracker.partial_of(&sid), None);
}

#[tokio::test]
async fn partial_of_treats_whitespace_as_none() {
    let tracker = tracker();
    let sid = sid();
    tracker.observe(&sid, &chunk("msg_1", "  \n "));
    assert_eq!(tracker.partial_of(&sid), None);
}

#[tokio::test]
async fn abort_emits_done_exactly_once_for_live_handle() {
    let sid = sid();
    let bus = crate::comms::EventBus::new();
    let mut rx = bus.subscribe(sid.clone());
    let tracker = BtwTracker::new(bus);
    let running = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });
    tracker.register(sid.clone(), BtwId::new(), running.abort_handle());

    tracker.abort(&sid, BtwEndReason::Cancelled);
    let (_, envelope) = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("Done must be emitted")
        .expect("channel open");
    assert!(matches!(
        envelope.event,
        Event::Btw(BtwEvent::Done {
            reason: BtwEndReason::Cancelled,
            ..
        })
    ));

    // 句柄已取走：第二次 abort 不再发（Done 恰好一次）。
    tracker.abort(&sid, BtwEndReason::Cancelled);
    tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
        .await
        .expect_err("no duplicate Done");
}

#[tokio::test]
async fn abort_emits_done_for_finished_handle_missing_complete() {
    // task 完成但 complete 未跑（异常滞留）：abort 抢到句柄即补终态——
    // 调用方不空等（抢删 = 终态归属；旧 is_finished 跳过语义反可能
    // 让调用方永远等不到 Done）。
    let sid = sid();
    let bus = crate::comms::EventBus::new();
    let mut rx = bus.subscribe(sid.clone());
    let tracker = BtwTracker::new(bus);
    let done = tokio::spawn(async {});
    let handle = done.abort_handle();
    done.await.unwrap();
    tracker.register(sid.clone(), BtwId::new(), handle);
    tracker.abort(&sid, BtwEndReason::Replaced);
    let (_, envelope) = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("abort must backfill Done for orphaned handle")
        .expect("channel open");
    assert!(matches!(
        envelope.event,
        Event::Btw(BtwEvent::Done {
            reason: BtwEndReason::Replaced,
            ..
        })
    ));
}

#[tokio::test]
async fn done_exactly_once_complete_wins_then_abort_noop() {
    let sid = sid();
    let bus = crate::comms::EventBus::new();
    let mut rx = bus.subscribe(sid.clone());
    let tracker = BtwTracker::new(bus);
    let rid = BtwId::new();
    let running = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });
    tracker.register(sid.clone(), rid.clone(), running.abort_handle());

    // 自然结束先抢删句柄 → abort 落空（Done 恰好一次的结构保证）。
    tracker.complete(&sid, &rid, BtwEndReason::Stop);
    tracker.abort(&sid, BtwEndReason::Cancelled);
    let (_, envelope) = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("Done must be emitted")
        .expect("channel open");
    assert!(matches!(
        envelope.event,
        Event::Btw(BtwEvent::Done {
            reason: BtwEndReason::Stop,
            ..
        })
    ));
    tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
        .await
        .expect_err("no second Done after abort lost the race");
    running.abort();
}

#[tokio::test]
async fn done_exactly_once_abort_wins_then_complete_noop() {
    let sid = sid();
    let bus = crate::comms::EventBus::new();
    let mut rx = bus.subscribe(sid.clone());
    let tracker = BtwTracker::new(bus);
    let rid = BtwId::new();
    let running = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    });
    tracker.register(sid.clone(), rid.clone(), running.abort_handle());

    // abort 先抢删 → 迟到的 complete 落空（task 被掐前可能刚好自然
    // 结束——is_finished 时序检查堵不住的窗口由抢删收口）。
    tracker.abort(&sid, BtwEndReason::Cancelled);
    tracker.complete(&sid, &rid, BtwEndReason::Stop);
    let (_, envelope) = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("Done must be emitted")
        .expect("channel open");
    assert!(matches!(
        envelope.event,
        Event::Btw(BtwEvent::Done {
            reason: BtwEndReason::Cancelled,
            ..
        })
    ));
    tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
        .await
        .expect_err("no second Done after complete lost the race");
}

// ── 快照组装 / 补全流 / 代际与锁 ─────────────────────────────────

#[test]
fn assemble_btw_history_prepends_system_and_drops_stale() {
    let history = vec![
        text_msg(Role::System, "旧 system"),
        text_msg(Role::User, "q"),
        text_msg(Role::Assistant, "a"),
    ];
    let assembled = assemble_btw_history("新 system".to_string(), &history);
    assert_eq!(
        roles_of(&assembled),
        [Role::System, Role::User, Role::Assistant]
    );
    assert_eq!(assembled[0].content[0].as_text().unwrap(), "新 system");
}

#[derive(Debug)]
struct FixedStreamProvider {
    items: Vec<crate::provider::ModelStreamItem>,
}

#[async_trait::async_trait]
impl Provider for FixedStreamProvider {
    fn name(&self) -> &str {
        "fixed-stream"
    }

    async fn stream(
        &self,
        _messages: &[Arc<Message>],
        _tools: &[Arc<ToolDefinition>],
        _config: &ModelConfig,
    ) -> Result<crate::provider::ModelStream, crate::provider::ProviderError> {
        Ok(Box::pin(futures::stream::iter(
            self.items.clone().into_iter().map(Ok),
        )))
    }
}

struct CollectSink(std::sync::Mutex<Vec<BtwEvent>>);

impl crate::comms::EventSink for CollectSink {
    fn emit(&self, envelope: Envelope) {
        if let Event::Btw(event) = envelope.event {
            self.0.lock().unwrap().push(event);
        }
    }
}

async fn run_with_items(
    items: Vec<crate::provider::ModelStreamItem>,
) -> (Vec<BtwEvent>, BtwEndReason) {
    let sink = Arc::new(CollectSink(std::sync::Mutex::new(Vec::new())));
    let history = vec![text_msg(Role::System, "sys"), text_msg(Role::User, "q")];
    let messages = build_btw_snapshot(&history, None, "问");
    let reason = run_btw_completion(
        sink.clone(),
        SessionId::from("sess_test".to_string()),
        BtwId::new(),
        Arc::new(FixedStreamProvider { items }),
        messages,
        Vec::new(),
        ModelConfig::default(),
    )
    .await;
    let events = sink.0.lock().unwrap().clone();
    (events, reason)
}

#[tokio::test]
async fn completion_streams_text_deltas_then_stop() {
    use crate::provider::ModelStreamItem;
    let (events, reason) = run_with_items(vec![
        ModelStreamItem::Chunk(ContentChunk::Text("你".to_string())),
        ModelStreamItem::Chunk(ContentChunk::Text("好".to_string())),
        ModelStreamItem::Complete,
    ])
    .await;
    assert!(
        matches!(
            &events[..],
            [
                BtwEvent::Delta { text: t1, .. },
                BtwEvent::Delta { text: t2, .. },
            ] if t1 == "你" && t2 == "好"
        ),
        "unexpected event stream: {events:?}"
    );
    assert_eq!(reason, BtwEndReason::Stop);
}

#[tokio::test]
async fn completion_pure_tool_use_ends_with_tool_use_reason() {
    use crate::provider::ModelStreamItem;
    let (events, reason) = run_with_items(vec![
        ModelStreamItem::ToolCall(crate::provider::ToolCallRequest {
            id: "c1".to_string(),
            name: "shell".to_string(),
            arguments: serde_json::json!({}),
        }),
        ModelStreamItem::Complete,
    ])
    .await;
    // 纯 tool_use 无文本：无 Delta，终态 ToolUse（客户端兜底"这需要正式提问"）。
    assert!(events.is_empty());
    assert_eq!(reason, BtwEndReason::ToolUse);
}

#[tokio::test]
async fn cancel_gen_bumps_and_reads_per_session() {
    let tracker = tracker();
    let sid_a = SessionId::from("sess_a".to_string());
    let sid_b = SessionId::from("sess_b".to_string());
    assert_eq!(tracker.cancel_gen(&sid_a), 0);
    tracker.bump_cancel_gen(&sid_a);
    tracker.bump_cancel_gen(&sid_a);
    assert_eq!(tracker.cancel_gen(&sid_a), 2);
    // 代际按会话隔离。
    assert_eq!(tracker.cancel_gen(&sid_b), 0);
}

#[tokio::test]
async fn start_lock_serializes_same_session_only() {
    let tracker = tracker();
    let sid_a = SessionId::from("sess_a".to_string());
    let sid_b = SessionId::from("sess_b".to_string());
    let guard_a = tracker.lock(&sid_a).await;
    // 同会话：第二把锁等不到（50ms 超时）。
    tokio::time::timeout(std::time::Duration::from_millis(50), tracker.lock(&sid_a))
        .await
        .expect_err("same session must serialize");
    // 跨会话：互不阻塞。
    let _guard_b = tokio::time::timeout(std::time::Duration::from_millis(50), tracker.lock(&sid_b))
        .await
        .expect("different session must not block");
    drop(guard_a);
    // 释放后可再获得。
    let _guard_a2 =
        tokio::time::timeout(std::time::Duration::from_millis(50), tracker.lock(&sid_a))
            .await
            .expect("released lock must be acquirable");
}

#[tokio::test]
async fn retain_keeps_in_flight_start_lock() {
    let tracker = tracker();
    let sid = sid();
    let _guard = tracker.lock(&sid).await;
    // 冷会话（keep=false）但锁在飞：retain 不得清锁（否则第二条旁问
    // 持新锁并发，abort/register 双双落空出孤儿流）。
    tracker.retain(|_| false);
    tokio::time::timeout(std::time::Duration::from_millis(50), tracker.lock(&sid))
        .await
        .expect_err("in-flight lock must survive retain");
}
