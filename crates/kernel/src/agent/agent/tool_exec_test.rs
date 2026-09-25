use super::{assign_message_ids, run_single_tool, RunSingleToolParams};
use crate::event::ToolEvent;
use crate::types::ToolCall;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn preassigned_message_id_is_preserved_by_tool_result() {
    let call = ToolCall {
        id: "tool-call".to_string(),
        name: "missing-tool".to_string(),
        arguments: json!({}),
    };
    let message_ids = assign_message_ids(std::slice::from_ref(&call));
    let message_id = message_ids[&call.id].clone();

    let result = run_single_tool(RunSingleToolParams {
        tool_opt: None,
        call_id: &call.id,
        call_name: &call.name,
        arguments: call.arguments,
        message_id: message_id.clone(),
        cancel_token: CancellationToken::new(),
        working_dir: std::path::PathBuf::from("."),
        session_id: "session".to_string(),
        turn: None,
        max_tool_output_length: 1024,
    })
    .await;

    assert_eq!(result.message_id, message_id);
    assert_eq!(result.message.id, message_id);
    assert_eq!(
        result.message.tool_call_id.as_deref(),
        Some(call.id.as_str())
    );
    match result.event {
        ToolEvent::End {
            message_id: event_message_id,
            tool_id,
            ..
        } => {
            assert_eq!(event_message_id, message_id);
            assert_eq!(tool_id, call.id);
        }
        event => panic!("expected tool end event, got {event:?}"),
    }
}

/// Mid-batch cancel: calls that already finished keep their real results;
/// every still-running call gets a synthesized cancelled result persisted
/// (call order, metadata-flagged), so the assistant→tool chain stays
/// complete and `sanitize` keeps the whole batch in context.
#[tokio::test]
async fn cancel_persists_cancelled_results_for_unfinished_calls() {
    use crate::agent::{Agent, AgentError, AgentShared, AgentSpawnArgs};
    use crate::tools::executor::CANCELLED_TOOL_OUTPUT_TEXT;
    use crate::tools::{Tool, ToolExecCtx};
    use crate::types::{Message, Result, Role, SessionId, ToolOutput, TOOL_CANCELLED_META_KEY};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Notify;

    struct FastTool(Arc<Notify>);

    #[async_trait]
    impl Tool for FastTool {
        fn name(&self) -> &'static str {
            "probe_fast"
        }
        fn desc(&self) -> &'static str {
            "returns immediately"
        }
        fn schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn exec(&self, _args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
            self.0.notify_one();
            Ok(ToolOutput::text("fast ok"))
        }
    }

    struct SlowTool(&'static str);

    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &'static str {
            self.0
        }
        fn desc(&self) -> &'static str {
            "never completes"
        }
        fn schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn exec(&self, _args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
            std::future::pending::<()>().await;
            unreachable!("slow tool never completes")
        }
    }

    let shared = Arc::new(AgentShared::new(
        Arc::new(BTreeMap::new()),
        "test".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
    ));
    let working_dir = tempfile::tempdir().unwrap();
    let args = AgentSpawnArgs {
        base_prompt: "test".to_string(),
        skills: Vec::new(),
        history: Vec::new(),
        session_id: SessionId::new().to_string(),
        parent_session_id: None,
        max_iterations: 1,
        tool_loop_guard: crate::agent::LoopGuard::default(),
        working_dir: working_dir.path().to_path_buf(),
        cancel_token: None,
        tool_flags: crate::tools::ToolFlags::new(false),
        file_state_store: None,
        tool_blocklist: Vec::new(),
        max_tool_output_length: 1024,
        mailbox: Arc::new(crate::comms::Mailbox::new()),
        input_bus: None,
        ext_tools: Vec::new(),
    };
    let mut agent = Agent::new(&shared, args).await;

    let fast_done = Arc::new(Notify::new());
    agent.tool_registry.register(FastTool(fast_done.clone()));
    agent.tool_registry.register(SlowTool("probe_slow_a"));
    agent.tool_registry.register(SlowTool("probe_slow_b"));

    let mut assistant = Message::assistant("running tools");
    assistant.tool_calls = Some(vec![
        ToolCall {
            id: "call-fast".to_string(),
            name: "probe_fast".to_string(),
            arguments: json!({}),
        },
        ToolCall {
            id: "call-slow-a".to_string(),
            name: "probe_slow_a".to_string(),
            arguments: json!({}),
        },
        ToolCall {
            id: "call-slow-b".to_string(),
            name: "probe_slow_b".to_string(),
            arguments: json!({}),
        },
    ]);
    agent.message_buffer.push_arc(Arc::new(assistant));

    // Cancel once the fast tool has run and its result had a beat to land;
    // the two slow tools are still pending at that point. The margin is
    // generous: the fast result must finish wrapping (build_tool_result +
    // image normalize) and be persisted before the cancel lands, or the
    // fast assertions below flake on a stalled runner.
    let cancel = agent.cancel_token.clone();
    tokio::spawn(async move {
        fast_done.notified().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    });

    let result = tokio::time::timeout(Duration::from_secs(10), agent.handle_execute_tool())
        .await
        .expect("tool batch should settle promptly after cancel");
    assert!(
        matches!(result, Err(AgentError::Cancelled(_))),
        "expected cancellation, got {result:?}"
    );

    // Buffer: one tool result per call — the fast one first (completed
    // before the cancel), then cancelled placeholders in call order.
    let tool_call_ids: Vec<&str> = agent
        .message_buffer
        .messages()
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.tool_call_id.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(tool_call_ids, ["call-fast", "call-slow-a", "call-slow-b"]);

    let tool_msg = |id: &str| {
        agent
            .message_buffer
            .messages()
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("missing tool result for {id}"))
            .clone()
    };
    let text_of = |m: &Message| match m.content.first() {
        Some(crate::types::ContentBlock::Text { text }) => text.clone(),
        other => panic!("expected text content, got {other:?}"),
    };

    // Completed call: real result, no cancelled flag.
    let fast = tool_msg("call-fast");
    assert!(text_of(&fast).contains("fast ok"));
    assert!(!fast
        .metadata
        .as_ref()
        .is_some_and(|md| md.contains_key(TOOL_CANCELLED_META_KEY)));

    // Unfinished calls: synthesized cancelled results, metadata-flagged.
    for id in ["call-slow-a", "call-slow-b"] {
        let msg = tool_msg(id);
        assert_eq!(
            text_of(&msg),
            format!("Error: {CANCELLED_TOOL_OUTPUT_TEXT}")
        );
        assert_eq!(
            msg.metadata
                .as_ref()
                .and_then(|md| md.get(TOOL_CANCELLED_META_KEY))
                .map(String::as_str),
            Some("true"),
            "cancelled flag missing on {id}"
        );
    }

    // Batch looks fully answered: a respawn finds nothing to re-execute…
    let (_, pending) = agent.pending_tool_calls().expect("tool batch present");
    assert!(
        pending.is_empty(),
        "cancelled calls must not be re-executed"
    );

    // …and the complete chain survives the pre-provider sanitize pass.
    let before = agent.message_buffer.messages().len();
    agent.message_buffer.sanitize();
    assert_eq!(agent.message_buffer.messages().len(), before);
}

/// respawn 关账后的历史不再触发工具重放（2026-09-11 评审 S3 锁定）：
/// dangling 批补齐 cancelled 结果后 `pending_tool_calls` 返回空
/// pending（pure recovery）——「副作用操作默认不做」的语义防回退。
#[tokio::test]
async fn closed_out_history_does_not_re_execute_batch() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs, MessageBuffer};
    use crate::types::{Message, SessionId};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    async fn spawn_with_history(history: Vec<Arc<Message>>) -> Agent {
        let shared = Arc::new(AgentShared::new(
            Arc::new(BTreeMap::new()),
            "test".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
        ));
        let working_dir = tempfile::tempdir().unwrap();
        let args = AgentSpawnArgs {
            base_prompt: "test".to_string(),
            skills: Vec::new(),
            history,
            session_id: SessionId::new().to_string(),
            parent_session_id: None,
            max_iterations: 1,
            tool_loop_guard: crate::agent::LoopGuard::default(),
            working_dir: working_dir.path().to_path_buf(),
            cancel_token: None,
            tool_flags: crate::tools::ToolFlags::new(false),
            file_state_store: None,
            tool_blocklist: Vec::new(),
            max_tool_output_length: 1024,
            mailbox: Arc::new(crate::comms::Mailbox::new()),
            input_bus: None,
            ext_tools: Vec::new(),
        };
        Agent::new(&shared, args).await
    }

    let mut assistant = Message::assistant("running tools");
    assistant.tool_calls = Some(vec![ToolCall {
        id: "call-1".to_string(),
        name: "probe".to_string(),
        arguments: json!({}),
    }]);
    let dangling = vec![Arc::new(Message::user("go")), Arc::new(assistant)];

    // 对照：未关账的 dangling 批会被当作待执行（重放行为——关账
    // 要消除的正是它）。
    let agent = spawn_with_history(dangling.clone()).await;
    let (_, pending) = agent.pending_tool_calls().expect("dangling batch detected");
    assert_eq!(pending.len(), 1, "raw dangling batch would re-execute");

    // 关账后：pending 为空，pure recovery，不重跑。
    let (closed, synthesized) = MessageBuffer::close_dangling_tool_batches(&dangling, 1024);
    assert_eq!(synthesized.len(), 1);
    let agent = spawn_with_history(closed).await;
    let (_, pending) = agent.pending_tool_calls().expect("batch still present");
    assert!(pending.is_empty(), "closed-out batch must not re-execute");
}

/// 循环哨兵端到端：同一调用（参数与结果均相同）连续重复——第 2 次
/// 注入 user 警告（L1，turn 继续），第 3 次熔断（L2，记
/// `StopReason::ToolLoop` 走 `WindingDown` 收尾）。
#[tokio::test]
async fn identical_call_loop_warns_then_breaks_turn() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs, AgentState};
    use crate::event::StopReason;
    use crate::tools::{Tool, ToolExecCtx};
    use crate::types::{Message, Result, Role, SessionId, ToolOutput};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn desc(&self) -> &'static str {
            "constant output"
        }
        fn schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn exec(&self, _args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
            Ok(ToolOutput::text("constant output"))
        }
    }

    async fn run_round(
        agent: &mut Agent,
        tag: &str,
    ) -> std::result::Result<(), crate::agent::AgentError> {
        let mut assistant = Message::assistant("calling echo");
        assistant.tool_calls = Some(vec![ToolCall {
            id: format!("call-{tag}"),
            name: "echo".to_string(),
            arguments: json!({}),
        }]);
        agent.message_buffer.push_arc(Arc::new(assistant));
        agent.handle_execute_tool().await
    }

    let shared = Arc::new(AgentShared::new(
        Arc::new(BTreeMap::new()),
        "test".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
    ));
    let working_dir = tempfile::tempdir().unwrap();
    let args = AgentSpawnArgs {
        base_prompt: "test".to_string(),
        skills: Vec::new(),
        history: Vec::new(),
        session_id: SessionId::new().to_string(),
        parent_session_id: None,
        max_iterations: 100,
        tool_loop_guard: crate::agent::LoopGuard::default(),
        working_dir: working_dir.path().to_path_buf(),
        cancel_token: None,
        tool_flags: crate::tools::ToolFlags::new(false),
        file_state_store: None,
        tool_blocklist: Vec::new(),
        max_tool_output_length: 1024,
        mailbox: Arc::new(crate::comms::Mailbox::new()),
        input_bus: None,
        ext_tools: Vec::new(),
    };
    let mut agent = Agent::new(&shared, args).await;
    agent.tool_registry.register(EchoTool);

    // 第 1 次：正常执行，转 Streaming，无警告。
    run_round(&mut agent, "1").await.expect("round 1");
    assert_eq!(agent.context.current_state(), AgentState::Streaming);
    assert!(
        !agent
            .message_buffer
            .messages()
            .iter()
            .any(|m| m.role == Role::User),
        "first call must not trigger a warning"
    );

    // 第 2 次（identical）：L1 警告注入，turn 继续。
    run_round(&mut agent, "2").await.expect("round 2");
    assert_eq!(agent.context.current_state(), AgentState::Streaming);
    let warning = agent
        .message_buffer
        .messages()
        .iter()
        .filter(|m| m.role == Role::User)
        .map(|m| match m.content.first() {
            Some(crate::types::ContentBlock::Text { text }) => text.clone(),
            other => panic!("expected text content, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(warning.len(), 1, "exactly one loop warning injected");
    assert!(warning[0].contains("[loop guard]"));
    assert!(warning[0].contains("`echo`"));

    // 警告带 metadata 标记：哨兵扫描对它透明（否则警告自身成了
    // turn 边界，L1→L2 梯子断裂），transcript/UI 可辨识。
    let warn_msg = agent
        .message_buffer
        .messages()
        .iter()
        .find(|m| m.role == Role::User)
        .expect("warning message");
    assert_eq!(
        warn_msg
            .metadata
            .as_ref()
            .and_then(|md| md.get(crate::types::LOOP_GUARD_META_KEY))
            .map(String::as_str),
        Some("true")
    );

    // 警告注入在批结果之后：has_user_after 守卫生效，respawn 不会
    // 重放这个已收尾的批（与中断标记同语义）。
    assert!(
        agent.pending_tool_calls().is_none(),
        "warning injection closes the batch against replay"
    );

    // 第 3 次（identical）：L2 熔断，记 ToolLoop，走 WindingDown。
    run_round(&mut agent, "3").await.expect("round 3");
    assert_eq!(agent.context.current_state(), AgentState::WindingDown);
    assert_eq!(
        agent.last_stop_reason,
        Some(StopReason::ToolLoop {
            tool: "echo".to_string(),
            count: 3,
        })
    );
}

/// 结果变化的重复调用（轮询/改后重读）不触发哨兵。
#[tokio::test]
async fn fresh_results_do_not_trip_the_guard() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs, AgentState};
    use crate::tools::{Tool, ToolExecCtx};
    use crate::types::{Message, Result, Role, SessionId, ToolOutput};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CounterTool(Arc<AtomicUsize>);

    #[async_trait]
    impl Tool for CounterTool {
        fn name(&self) -> &'static str {
            "counter"
        }
        fn desc(&self) -> &'static str {
            "fresh output every call"
        }
        fn schema(&self) -> Value {
            json!({"type": "object"})
        }
        async fn exec(&self, _args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::text(format!("count {n}")))
        }
    }

    let shared = Arc::new(AgentShared::new(
        Arc::new(BTreeMap::new()),
        "test".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
    ));
    let working_dir = tempfile::tempdir().unwrap();
    let args = AgentSpawnArgs {
        base_prompt: "test".to_string(),
        skills: Vec::new(),
        history: Vec::new(),
        session_id: SessionId::new().to_string(),
        parent_session_id: None,
        max_iterations: 100,
        tool_loop_guard: crate::agent::LoopGuard::default(),
        working_dir: working_dir.path().to_path_buf(),
        cancel_token: None,
        tool_flags: crate::tools::ToolFlags::new(false),
        file_state_store: None,
        tool_blocklist: Vec::new(),
        max_tool_output_length: 1024,
        mailbox: Arc::new(crate::comms::Mailbox::new()),
        input_bus: None,
        ext_tools: Vec::new(),
    };
    let mut agent = Agent::new(&shared, args).await;
    agent
        .tool_registry
        .register(CounterTool(Arc::new(AtomicUsize::new(0))));

    for tag in ["1", "2", "3", "4"] {
        let mut assistant = Message::assistant("polling");
        assistant.tool_calls = Some(vec![ToolCall {
            id: format!("call-{tag}"),
            name: "counter".to_string(),
            arguments: json!({}),
        }]);
        agent.message_buffer.push_arc(Arc::new(assistant));
        agent.handle_execute_tool().await.expect("round");
        assert_eq!(
            agent.context.current_state(),
            AgentState::Streaming,
            "round {tag}: fresh results must keep the guard silent"
        );
    }
    assert!(
        !agent
            .message_buffer
            .messages()
            .iter()
            .any(|m| m.role == Role::User),
        "no warning for fresh results"
    );
}
