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

/// 注入的 user 提醒（L1 产物）不打断 streak——模型无视警告再
/// 复读一次即熔断。
#[test]
fn injected_warning_does_not_reset_streak() {
    let mut messages = concat(vec![
        round("1", "probe", json!({}), "same"),
        round("2", "probe", json!({}), "same"),
    ]);
    messages.push(Arc::new(Message::user("[loop guard] stop retrying")));
    messages.extend(round("3", "probe", json!({}), "same"));
    assert_eq!(
        detect(&messages, GUARD),
        LoopSignal::Break {
            tool: "probe".to_string(),
            streak: 3,
        }
    );
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
