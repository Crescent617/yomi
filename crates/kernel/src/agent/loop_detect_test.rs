use super::{detect, LoopGuard, LoopSignal};
use crate::types::{ContentBlock, Message, ToolCall};
use serde_json::{json, Value};
use std::sync::Arc;

const GUARD: LoopGuard = LoopGuard {
    warn_at: 2,
    break_at: 3,
};

fn batch(id_tag: &str, name: &str, args: Value) -> Arc<Message> {
    let mut msg = Message::assistant("calling tools");
    msg.tool_calls = Some(vec![ToolCall {
        id: format!("call-{id_tag}"),
        name: name.to_string(),
        arguments: args,
    }]);
    Arc::new(msg)
}

fn result(id_tag: &str, text: &str) -> Arc<Message> {
    let mut msg = Message {
        role: crate::types::Role::Tool,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        ..Default::default()
    };
    msg.tool_call_id = Some(format!("call-{id_tag}"));
    Arc::new(msg)
}

/// 一轮「调用 + 结果」，tag 区分轮次（call id 各轮唯一）。
fn round(tag: &str, name: &str, args: Value, output: &str) -> Vec<Arc<Message>> {
    vec![batch(tag, name, args), result(tag, output)]
}

fn concat(rounds: Vec<Vec<Arc<Message>>>) -> Vec<Arc<Message>> {
    rounds.into_iter().flatten().collect()
}

#[test]
fn single_batch_is_clean() {
    let messages = concat(vec![round("1", "probe", json!({"x": 1}), "same")]);
    assert_eq!(detect(&messages, GUARD), LoopSignal::None);
}

#[test]
fn second_identical_call_warns() {
    let messages = concat(vec![
        round("1", "probe", json!({"x": 1}), "same"),
        round("2", "probe", json!({"x": 1}), "same"),
    ]);
    assert_eq!(
        detect(&messages, GUARD),
        LoopSignal::Warn {
            tool: "probe".to_string(),
            streak: 2,
        }
    );
}

#[test]
fn third_identical_call_breaks() {
    let messages = concat(vec![
        round("1", "probe", json!({"x": 1}), "same"),
        round("2", "probe", json!({"x": 1}), "same"),
        round("3", "probe", json!({"x": 1}), "same"),
    ]);
    assert_eq!(
        detect(&messages, GUARD),
        LoopSignal::Break {
            tool: "probe".to_string(),
            streak: 3,
        }
    );
}

/// 防误伤核心：A,B,A,B 交错（轮询节奏）形不成连续 streak。
#[test]
fn interleaved_polling_never_trips() {
    let messages = concat(vec![
        round("1", "wait", json!({"s": 30}), "ok"),
        round("2", "check", json!({}), "pending"),
        round("3", "wait", json!({"s": 30}), "ok"),
        round("4", "check", json!({}), "pending"),
        round("5", "wait", json!({"s": 30}), "ok"),
    ]);
    assert_eq!(detect(&messages, GUARD), LoopSignal::None);
}

/// 结果不同的重复调用（改后重读）不是循环。
#[test]
fn same_call_with_fresh_results_is_clean() {
    let messages = concat(vec![
        round("1", "probe", json!({"x": 1}), "v1"),
        round("2", "probe", json!({"x": 1}), "v2"),
        round("3", "probe", json!({"x": 1}), "v3"),
    ]);
    assert_eq!(detect(&messages, GUARD), LoopSignal::None);
}

#[test]
fn different_args_is_clean() {
    let messages = concat(vec![
        round("1", "probe", json!({"x": 1}), "same"),
        round("2", "probe", json!({"x": 2}), "same"),
        round("3", "probe", json!({"x": 3}), "same"),
    ]);
    assert_eq!(detect(&messages, GUARD), LoopSignal::None);
}

/// canonical 化：参数 key 序不同仍是同一调用。
#[test]
fn reordered_arg_keys_still_match() {
    let messages = concat(vec![
        round("1", "probe", json!({"a": 1, "b": {"x": 1, "y": 2}}), "same"),
        round("2", "probe", json!({"b": {"y": 2, "x": 1}, "a": 1}), "same"),
    ]);
    assert!(matches!(
        detect(&messages, GUARD),
        LoopSignal::Warn { streak: 2, .. }
    ));
}

/// 哨兵注入的警告（turn-internal 标记）对扫描透明——模型无视警告
/// 再复读一次即熔断，L1→L2 梯子不断。
#[test]
fn injected_warning_does_not_reset_streak() {
    let mut messages = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
    ]);
    messages.push(Arc::new(Message::user_turn_internal(
        "[loop guard] stop retrying",
    )));
    messages.extend(round("3", "probe", json!({}), "same"));
    assert_eq!(
        detect(&messages, GUARD),
        LoopSignal::Break {
            tool: "probe".to_string(),
            streak: 3,
        }
    );
}

/// auto-continue 注入的 "continue" 同走 turn-internal 标记——循环
/// 跨越它不重置（与 `max_iterations` 不因它重置的语义对齐）。
#[test]
fn auto_continue_message_is_transparent() {
    let mut messages = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
    ]);
    messages.push(Arc::new(Message::user_turn_internal("continue")));
    messages.extend(round("3", "probe", json!({}), "same"));
    assert!(matches!(
        detect(&messages, GUARD),
        LoopSignal::Break { streak: 3, .. }
    ));
}

/// turn 起跑的 steer（Idle 臂注入，无 turn-internal 标记）是边界。
/// mid-turn steer 由注入点打标记（`inject_user_message` 按
/// `current_turn.is_some()` 判定），不在本函数判定面内。
#[test]
fn steer_marks_boundary() {
    let mut messages = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
    ]);
    let mut steer = Message::user("also do X");
    steer.metadata = Some(std::collections::HashMap::from([(
        crate::types::IS_STEER_META_KEY.to_string(),
        "true".to_string(),
    )]));
    messages.push(Arc::new(steer));
    messages.extend(round("3", "probe", json!({}), "same"));
    assert_eq!(detect(&messages, GUARD), LoopSignal::None);
}

/// 跨 turn 不泄漏：真实 user 消息是扫描硬边界——上一 turn 的两次
/// 复读（已警告）不算进这一 turn 的第一次相同调用。
#[test]
fn real_user_message_marks_turn_boundary() {
    let mut messages = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
    ]);
    // 上一 turn 结束（含最终文本回答），用户开启新 turn。
    messages.push(Arc::new(Message::assistant("done")));
    messages.push(Arc::new(Message::user("check it again")));
    messages.extend(round("3", "probe", json!({}), "same"));
    assert_eq!(
        detect(&messages, GUARD),
        LoopSignal::None,
        "streak must not leak across turns"
    );

    // 边界后重新计：本 turn 内再复读一次 → 警告而非熔断。
    messages.extend(round("4", "probe", json!({}), "same"));
    assert_eq!(
        detect(&messages, GUARD),
        LoopSignal::Warn {
            tool: "probe".to_string(),
            streak: 2,
        }
    );
}

/// 中断标记（`[Request interrupted by user]`，无哨兵标记）同样
/// 是边界——cancel 之后的重试重新计数。
#[test]
fn interruption_marker_marks_boundary() {
    let mut messages = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
    ]);
    let mut marker = Message::user("[Request interrupted by user]");
    marker.metadata = Some(std::collections::HashMap::from([(
        crate::types::INTERRUPTED_META_KEY.to_string(),
        "true".to_string(),
    )]));
    messages.push(Arc::new(marker));
    messages.extend(round("3", "probe", json!({}), "same"));
    assert_eq!(detect(&messages, GUARD), LoopSignal::None);
}

#[test]
fn disabled_guard_stays_silent() {
    let messages = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
        round("3", "probe", json!({}), "same"),
    ]);
    let off = LoopGuard {
        warn_at: 2,
        break_at: 0,
    };
    assert_eq!(detect(&messages, off), LoopSignal::None);
}

#[test]
fn warn_can_be_disabled_independently() {
    let no_warn = LoopGuard {
        warn_at: 0,
        break_at: 3,
    };
    let two = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
    ]);
    assert_eq!(detect(&two, no_warn), LoopSignal::None);
    let three = concat(vec![two, round("3", "probe", json!({}), "same")]);
    assert!(matches!(
        detect(&three, no_warn),
        LoopSignal::Break { streak: 3, .. }
    ));
}

/// 阈值 1 是退化配置，按 2 处理：break=1 不熔断单次调用，warn=1
/// 不发 "called 1 times" 的无意义警告。
#[test]
fn threshold_one_is_clamped_to_two() {
    let degenerate = LoopGuard {
        warn_at: 1,
        break_at: 1,
    };
    let one = concat(vec![round("1", "probe", json!({}), "same")]);
    assert_eq!(detect(&one, degenerate), LoopSignal::None);
    let two = concat(vec![one, round("2", "probe", json!({}), "same")]);
    assert_eq!(
        detect(&two, degenerate),
        LoopSignal::Break {
            tool: "probe".to_string(),
            streak: 2,
        }
    );
}

/// 一批多个调用时，信号指名真正在复读的那个工具。
#[test]
fn multi_call_batch_blames_the_repeating_tool() {
    let round_pair = |tag: &str, check_out: &str| {
        let mut msg = Message::assistant("calling tools");
        msg.tool_calls = Some(vec![
            ToolCall {
                id: format!("call-{tag}-a"),
                name: "stuck".to_string(),
                arguments: json!({}),
            },
            ToolCall {
                id: format!("call-{tag}-b"),
                name: "fresh".to_string(),
                arguments: json!({"tag": tag}),
            },
        ]);
        vec![
            Arc::new(msg),
            result(&format!("{tag}-a"), "same"),
            result(&format!("{tag}-b"), check_out),
        ]
    };
    let messages = concat(vec![round_pair("1", "r1"), round_pair("2", "r2")]);
    assert_eq!(
        detect(&messages, GUARD),
        LoopSignal::Warn {
            tool: "stuck".to_string(),
            streak: 2,
        }
    );
}

/// 悬挂调用（无结果，如 mid-batch kill）不参与判定。
#[test]
fn dangling_call_is_ignored() {
    let mut messages = concat(vec![round("1", "probe", json!({}), "same")]);
    messages.push(batch("2", "probe", json!({}))); // 无结果
    assert_eq!(detect(&messages, GUARD), LoopSignal::None);
}

/// 文本周（assistant 无 `tool_calls`）不截断连续工具批的 streak。
#[test]
fn text_only_assistant_turn_does_not_reset_streak() {
    let mut messages = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
    ]);
    messages.push(Arc::new(Message::assistant("let me think again")));
    messages.extend(round("3", "probe", json!({}), "same"));
    assert!(matches!(
        detect(&messages, GUARD),
        LoopSignal::Break { streak: 3, .. }
    ));
}
