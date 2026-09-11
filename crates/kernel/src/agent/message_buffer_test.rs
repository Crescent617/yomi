use super::*;
use crate::types::{ContentBlock, Message, Role, ToolCall};
use chrono::Utc;

fn create_assistant_with_tools(tool_ids: Vec<&str>) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "calling tools".to_string(),
        }],
        tool_calls: Some(
            tool_ids
                .into_iter()
                .map(|tid| ToolCall {
                    id: tid.to_string(),
                    name: "test_tool".to_string(),
                    arguments: serde_json::json!({}),
                })
                .collect(),
        ),
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    }
}

fn create_tool_response(tool_call_id: &str) -> Message {
    Message {
        role: Role::Tool,
        content: vec![ContentBlock::Text {
            text: "result".to_string(),
        }],
        tool_calls: None,
        tool_call_id: Some(tool_call_id.to_string()),
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    }
}

fn create_user_message(content: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: content.to_string(),
        }],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    }
}

#[test]
fn test_valid_chain_kept() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.messages()[0].role, Role::Assistant);
    assert_eq!(buffer.messages()[1].role, Role::Tool);
}

#[test]
fn test_multiple_tools_kept() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1", "t2"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_tool_response("t2"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 3);
}

#[test]
fn test_interrupted_chain_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_user_message("interrupt"));
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 1);
    assert_eq!(buffer.messages()[0].role, Role::User);
}

#[test]
fn test_orphan_tool_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 0);
}

#[test]
fn test_missing_tool_response_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1", "t2"]));
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 0);
}

#[test]
fn test_extra_tool_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_tool_response("extra"));

    buffer.sanitize();

    // Only the orphan extra tool is removed, valid chain is kept
    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.messages()[0].role, Role::Assistant);
    assert_eq!(buffer.messages()[1].role, Role::Tool);
}

#[test]
fn test_wrong_tool_id_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t2"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 0);
}

#[test]
fn test_multiple_valid_chains() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_assistant_with_tools(vec!["t2"]));
    buffer.push(create_tool_response("t2"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 4);
}

#[test]
fn test_mixed_chains() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_assistant_with_tools(vec!["t2"]));
    buffer.push(create_user_message("interrupt"));
    buffer.push(create_tool_response("t2"));
    buffer.push(create_tool_response("orphan"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 3);
    assert_eq!(buffer.messages()[0].role, Role::Assistant);
    assert_eq!(buffer.messages()[1].role, Role::Tool);
    assert_eq!(buffer.messages()[2].role, Role::User);
}

#[test]
fn test_empty_buffer() {
    let mut buffer = MessageBuffer::new();
    buffer.sanitize();
    assert_eq!(buffer.len(), 0);
}

#[test]
fn test_assistant_without_tools() {
    let mut buffer = MessageBuffer::new();
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
        }],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });
    buffer.push(create_user_message("response"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 2);
}

#[test]
fn test_duplicate_tool_response_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(create_tool_response("t1"));
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    // Only the duplicate tool response is removed, valid chain is kept
    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.messages()[0].role, Role::Assistant);
    assert_eq!(buffer.messages()[1].role, Role::Tool);
}

/// 空 completion 毒化自愈：无内容、无 tool_calls 的 assistant 消息（模型
/// 抽风落盘的毒）在 sanitize 时被摘除，其余消息原样保留——已中毒 session
/// 升级后下一轮自动康复，不必手工删 jsonl。
#[test]
fn test_empty_assistant_poison_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_user_message("before"));
    // The poison shape: content == [], tool_calls == None (metadata-only
    // assistant persisted by the pre-fix guard).
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });
    buffer.push(create_user_message("after"));

    buffer.sanitize();

    let roles: Vec<_> = buffer.messages().iter().map(|m| m.role).collect();
    assert_eq!(roles, vec![Role::User, Role::User]);
}

#[test]
fn test_nonempty_assistant_without_tools_kept() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_user_message("hi"));
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "answer".to_string(),
        }],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });

    buffer.sanitize();

    assert_eq!(buffer.messages().len(), 2);
    assert_eq!(buffer.messages()[1].role, Role::Assistant);
}

/// 边界 pin：仅含 thinking 的 assistant（content 非空、无 tool_calls）不是
/// 毒，sanitize 必须保留——防止未来"清理空消息"类重构把规则放宽误伤。
#[test]
fn test_thinking_only_assistant_kept() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_user_message("question"));
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Thinking {
            thinking: "reasoning".to_string(),
            signature: Some("sig".to_string()),
        }],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });

    buffer.sanitize();

    assert_eq!(buffer.len(), 2);
    assert_eq!(buffer.messages()[1].role, Role::Assistant);
}

/// 新规则与链式清理同轮交互：[带调用的 assistant, 空毒消息, 孤儿 tool]
/// 三者全部摘除——空毒使前一条链断裂，tool 本就成为孤儿。
#[test]
fn test_poison_between_chain_and_tool_removes_all() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1"]));
    buffer.push(Message {
        role: Role::Assistant,
        content: vec![],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 0);
}

/// Internal 占位消息（subagent metadata，tool start 时由 conductor 持久
/// 化进 jsonl）对链透明：交错在 assistant 与结果之间、或结果与结果之
/// 间都不断链——respawn 后 subagent 工具链不再被整组抹掉。
#[test]
fn test_internal_placeholder_transparent_to_chain() {
    let internal = || Message {
        role: Role::Internal,
        content: vec![],
        tool_calls: None,
        tool_call_id: Some("t1".to_string()),
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    };

    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1", "t2"]));
    buffer.push(internal());
    buffer.push(create_tool_response("t1"));
    buffer.push(internal());
    buffer.push(create_tool_response("t2"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 5, "Internal must not break the tool chain");
    assert_eq!(buffer.messages()[1].role, Role::Internal);
    assert_eq!(buffer.messages()[3].role, Role::Internal);
}

/// 链不完整时照常剔除 assistant 与已收集结果——Internal 不陪葬（不是
/// 链成员，留给 UI 重放）。
#[test]
fn test_internal_kept_when_chain_removed() {
    let mut buffer = MessageBuffer::new();
    buffer.push(create_assistant_with_tools(vec!["t1", "t2"]));
    buffer.push(Message {
        role: Role::Internal,
        content: vec![],
        tool_calls: None,
        tool_call_id: None,
        created_at: Utc::now(),
        token_usage: None,
        ..Default::default()
    });
    buffer.push(create_tool_response("t1"));

    buffer.sanitize();

    assert_eq!(buffer.len(), 1);
    assert_eq!(buffer.messages()[0].role, Role::Internal);
}

// ── close_dangling_tool_batches ─────────────────────────────────────────────

fn arcs(msgs: Vec<Message>) -> Vec<Arc<Message>> {
    msgs.into_iter().map(Arc::new).collect()
}

fn internal_message() -> Message {
    Message {
        role: Role::Internal,
        content: vec![ContentBlock::Text {
            text: "meta".to_string(),
        }],
        ..Default::default()
    }
}

#[test]
fn close_dangling_noop_on_complete_batch() {
    let history = arcs(vec![
        create_user_message("go"),
        create_assistant_with_tools(vec!["t1"]),
        create_tool_response("t1"),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert!(synthesized.is_empty());
    assert_eq!(out.len(), history.len());
}

#[test]
fn close_dangling_appends_cancelled_at_end() {
    let history = arcs(vec![
        create_user_message("go"),
        create_assistant_with_tools(vec!["t1", "t2"]),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert_eq!(synthesized.len(), 2);
    assert_eq!(out.len(), 4);
    // 合成结果紧跟 assistant 批，顺序与调用一致。
    assert_eq!(out[2].role, Role::Tool);
    assert_eq!(out[2].tool_call_id.as_deref(), Some("t1"));
    assert_eq!(out[3].tool_call_id.as_deref(), Some("t2"));
    // cancelled 格式：metadata flag + error 文本。
    let meta = out[2].metadata.as_ref().unwrap();
    assert_eq!(
        meta.get(crate::types::TOOL_CANCELLED_META_KEY)
            .map(String::as_str),
        Some("true")
    );
    let ContentBlock::Text { text } = &out[2].content[0] else {
        panic!("text block expected");
    };
    assert!(text.contains("Tool execution cancelled"), "{text}");
    // 落库列表与历史内嵌同内容同序。
    assert_eq!(synthesized[0].tool_call_id.as_deref(), Some("t1"));
    assert_eq!(synthesized[1].tool_call_id.as_deref(), Some("t2"));
    // 补齐后 sanitize 不再剔除该批。
    let view = MessageBuffer::sanitized_model_messages(&out);
    assert_eq!(view.len(), 4);
}

#[test]
fn close_dangling_partial_batch_only_synthesizes_missing() {
    let history = arcs(vec![
        create_assistant_with_tools(vec!["t1", "t2"]),
        create_tool_response("t1"),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert_eq!(synthesized.len(), 1);
    assert_eq!(synthesized[0].tool_call_id.as_deref(), Some("t2"));
    assert_eq!(out.len(), 3);
    assert_eq!(out[2].tool_call_id.as_deref(), Some("t2"));
}

#[test]
fn close_dangling_inserts_before_interruption_marker() {
    // 双丢场景形状：assistant 批后只有 marker（user），cancelled 结果
    // 缺失 → 合成结果插在 marker 之前（abort 的真实时序）。
    let history = arcs(vec![
        create_assistant_with_tools(vec!["t1"]),
        create_user_message("[interrupted: cancelled]"),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert_eq!(synthesized.len(), 1);
    assert_eq!(out.len(), 3);
    assert_eq!(out[1].role, Role::Tool);
    assert_eq!(out[2].role, Role::User);
}

#[test]
fn close_dangling_internal_is_transparent() {
    // Internal 不打断开放批：assistant → Internal（缺口）→ 关账发生
    // 在下一个断链消息前，Internal 原样保留。
    let history = arcs(vec![
        create_assistant_with_tools(vec!["t1"]),
        internal_message(),
        create_user_message("next"),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert_eq!(synthesized.len(), 1);
    assert_eq!(out.len(), 4);
    assert_eq!(out[1].role, Role::Internal);
    assert_eq!(out[2].role, Role::Tool);
    assert_eq!(out[3].role, Role::User);
}

#[test]
fn close_dangling_multiple_batches_each_closed() {
    let history = arcs(vec![
        create_assistant_with_tools(vec!["a1"]),
        create_user_message("u"),
        create_assistant_with_tools(vec!["b1", "b2"]),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert_eq!(synthesized.len(), 3);
    let ids: Vec<_> = synthesized
        .iter()
        .map(|m| m.tool_call_id.as_deref().unwrap())
        .collect();
    assert_eq!(ids, ["a1", "b1", "b2"]);
}

#[test]
fn close_dangling_idempotent_on_second_pass() {
    let history = arcs(vec![create_assistant_with_tools(vec!["t1"])]);
    let (once, first) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert_eq!(first.len(), 1);
    let (_, second) = MessageBuffer::close_dangling_tool_batches(&once, 1000);
    assert!(second.is_empty(), "second pass must be a no-op");
}

#[test]
fn close_dangling_leaves_orphan_tool_untouched() {
    // 孤儿 tool（无对应 assistant 批）不属于任何开放批：原样保留
    //（剔除它是 sanitize 的职责，关账不管）。
    let history = arcs(vec![
        create_tool_response("ghost"),
        create_assistant_with_tools(vec!["t1"]),
        create_tool_response("t1"),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert!(synthesized.is_empty());
    assert_eq!(out.len(), 3);
}

#[test]
fn close_dangling_migrates_late_results_instead_of_resynthesizing() {
    // 跨 respawn 布局：上轮关账的合成结果 append 在文件尾（marker
    // 之后）。本轮必须迁移它回批内而非再合成——否则文件每轮 +N。
    let first_pass = arcs(vec![
        create_user_message("go"),
        create_assistant_with_tools(vec!["t1"]),
        create_user_message("[interrupted: cancelled]"),
    ]);
    let (_, synthesized_once) = MessageBuffer::close_dangling_tool_batches(&first_pass, 1000);
    assert_eq!(synthesized_once.len(), 1);
    // 模拟落盘布局：原历史（缺口）+ 合成结果 append 尾。
    let mut persisted = first_pass.clone();
    persisted.push(Arc::new(synthesized_once.into_iter().next().unwrap()));

    let (closed_twice, synthesized_twice) =
        MessageBuffer::close_dangling_tool_batches(&persisted, 1000);
    assert!(
        synthesized_twice.is_empty(),
        "late result must be migrated, not re-synthesized: {synthesized_twice:?}"
    );
    // 迁移后内存视图：tool 结果回到批内、marker 之前。
    assert_eq!(closed_twice.len(), 4);
    assert_eq!(closed_twice[1].role, Role::Assistant);
    assert_eq!(closed_twice[2].role, Role::Tool);
    assert_eq!(closed_twice[2].tool_call_id.as_deref(), Some("t1"));
    assert_eq!(closed_twice[3].role, Role::User);
    // sanitize 视图完整（无剔除）。
    assert_eq!(
        MessageBuffer::sanitized_model_messages(&closed_twice).len(),
        4
    );
}

#[test]
fn close_dangling_migrates_real_result_written_after_marker() {
    // 总线乱序：真实结果落盘在 marker 之后——关账迁移真实结果，
    // 不合成 cancelled 覆盖它。
    let history = arcs(vec![
        create_assistant_with_tools(vec!["t1"]),
        create_user_message("[interrupted: cancelled]"),
        create_tool_response("t1"),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert!(synthesized.is_empty());
    assert_eq!(out.len(), 3);
    assert_eq!(out[1].role, Role::Tool);
    let ContentBlock::Text { text } = &out[1].content[0] else {
        panic!("text block expected");
    };
    assert_eq!(
        text, "result",
        "real result migrated, not cancelled: {text}"
    );
}

#[test]
fn close_dangling_migrates_result_appearing_before_its_batch() {
    // 病态乱序：tool 结果写在其 assistant 之前——迁移后顺序修正。
    let history = arcs(vec![
        create_tool_response("t1"),
        create_assistant_with_tools(vec!["t1"]),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert!(synthesized.is_empty());
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].role, Role::Assistant);
    assert_eq!(out[1].role, Role::Tool);
    assert_eq!(out[1].tool_call_id.as_deref(), Some("t1"));
}

#[test]
fn close_dangling_mixed_migrate_and_synthesize() {
    // 混合批：t1 真实结果原位、t2 结果迟到在文件尾、t3 真缺口——
    // 原位保留 + 迁移 + 合成各就其位。
    let history = arcs(vec![
        create_assistant_with_tools(vec!["t1", "t2", "t3"]),
        create_tool_response("t1"),
        create_user_message("[interrupted: cancelled]"),
        create_tool_response("t2"),
    ]);
    let (out, synthesized) = MessageBuffer::close_dangling_tool_batches(&history, 1000);
    assert_eq!(synthesized.len(), 1);
    assert_eq!(synthesized[0].tool_call_id.as_deref(), Some("t3"));
    // [assistant, t1, t2(迁移), t3(合成), marker]
    assert_eq!(out.len(), 5);
    let seq: Vec<_> = out.iter().map(|m| m.role).collect();
    assert_eq!(
        seq,
        [
            Role::Assistant,
            Role::Tool,
            Role::Tool,
            Role::Tool,
            Role::User
        ]
    );
    assert_eq!(out[2].tool_call_id.as_deref(), Some("t2"));
    assert_eq!(out[3].tool_call_id.as_deref(), Some("t3"));
}
