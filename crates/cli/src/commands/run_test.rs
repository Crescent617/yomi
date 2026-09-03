use super::*;
use crate::utils::combine_prompt_stdin;
use kernel::event::{ContentChunk, ErrorPhase};
use kernel::types::MessageId;

fn text_block(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_string(),
    }
}

fn user_message(text: &str) -> Event {
    Event::User(UserEvent::Message {
        message_id: MessageId::from("m1"),
        content: vec![text_block(text)],
    })
}

fn model_end(text: &str) -> Event {
    Event::Model(ModelEvent::End {
        message_id: MessageId::from("m2"),
        content: vec![text_block(text)],
    })
}

fn stopped(reason: StopReason) -> Event {
    Event::Agent(AgentEvent::Lifecycle {
        state: AgentStatus::Stopped { reason },
    })
}

fn running() -> Event {
    Event::Agent(AgentEvent::Lifecycle {
        state: AgentStatus::Running,
    })
}

fn state(prompt: &str) -> RunState {
    RunState::new(prompt.to_string(), false)
}

// ── clap parsing ─────────────────────────────────────────────────────

#[test]
fn parse_defaults() {
    let args = RunArgs::try_parse_from(["yomi-run", "hello", "world"]).unwrap();
    assert_eq!(args.prompt, vec!["hello", "world"]);
    assert_eq!(args.format, OutputFormat::Text);
    assert!(args.model.is_none());
    assert!(args.resume.is_none());
    assert!(args.fork.is_none());
    assert!(!args.yolo);
    assert!(!args.mode.bg);
    assert!(!args.mode.fg);
    assert!(!args.ephemeral);
    assert!(!args.verbose);
    assert!(args.timeout.is_none());
}

#[test]
fn parse_formats() {
    let args = RunArgs::try_parse_from(["yomi-run", "--format", "json", "hi"]).unwrap();
    assert_eq!(args.format, OutputFormat::Json);
    let args = RunArgs::try_parse_from(["yomi-run", "--format", "stream-json", "hi"]).unwrap();
    assert_eq!(args.format, OutputFormat::StreamJson);
    assert!(RunArgs::try_parse_from(["yomi-run", "--format", "yaml", "hi"]).is_err());
}

#[test]
fn parse_conflicts() {
    assert!(RunArgs::try_parse_from(["yomi-run", "--bg", "--fg", "hi"]).is_err());
    assert!(
        RunArgs::try_parse_from(["yomi-run", "--yolo", "--auto-approve", "safe", "hi"]).is_err()
    );
    assert!(RunArgs::try_parse_from(["yomi-run", "--resume", "--fork", "hi"]).is_err());
}

#[test]
fn parse_resume_fork_variants() {
    // A space-separated session id is consumed by the flag, never as prompt.
    let args = RunArgs::try_parse_from(["yomi-run", "--resume", "sess_1", "hi"]).unwrap();
    assert_eq!(args.resume.as_deref(), Some("sess_1"));
    assert_eq!(args.prompt, vec!["hi"]);
    let args = RunArgs::try_parse_from(["yomi-run", "--resume=sess_1", "hi"]).unwrap();
    assert_eq!(args.resume.as_deref(), Some("sess_1"));
    let args = RunArgs::try_parse_from(["yomi-run", "--last", "hi"]).unwrap();
    assert!(args.last);
    let args = RunArgs::try_parse_from(["yomi-run", "--fork", "sess_2", "hi"]).unwrap();
    assert_eq!(args.fork.as_deref(), Some("sess_2"));
    assert_eq!(args.prompt, vec!["hi"]);
    let args = RunArgs::try_parse_from(["yomi-run", "--fork-last", "hi"]).unwrap();
    assert!(args.fork_last);
}

#[test]
fn parse_session_flags_conflict() {
    assert!(RunArgs::try_parse_from(["yomi-run", "--resume", "a", "--last", "hi"]).is_err());
    assert!(RunArgs::try_parse_from(["yomi-run", "--resume", "a", "--fork", "b", "hi"]).is_err());
    assert!(RunArgs::try_parse_from(["yomi-run", "--last", "--fork-last", "hi"]).is_err());
    assert!(RunArgs::try_parse_from(["yomi-run", "--fork", "a", "--fork-last", "hi"]).is_err());
}

#[test]
fn parse_model_and_timeout() {
    let args =
        RunArgs::try_parse_from(["yomi-run", "-m", "claude-sonnet", "--timeout", "60", "hi"])
            .unwrap();
    assert_eq!(args.model.as_deref(), Some("claude-sonnet"));
    assert_eq!(args.timeout, Some(60));
}

// ── prompt assembly ──────────────────────────────────────────────────

#[test]
fn combine_prompt_and_stdin() {
    assert_eq!(combine_prompt_stdin(None, None), None);
    assert_eq!(
        combine_prompt_stdin(Some("p".into()), None),
        Some("p".to_string())
    );
    assert_eq!(
        combine_prompt_stdin(None, Some("s".into())),
        Some("s".to_string())
    );
    assert_eq!(
        combine_prompt_stdin(Some("p".into()), Some("s".into())),
        Some("p\n\n```\ns\n```".to_string())
    );
}

#[test]
fn prompt_from_parts_joins_args() {
    let p = prompt_from_parts(&["hello".into(), "world".into()], None).unwrap();
    assert_eq!(p, "hello world");
}

#[test]
fn prompt_from_parts_requires_content() {
    assert!(prompt_from_parts(&[], None).is_err());
    assert!(prompt_from_parts(&["  ".into()], None).is_err());
    assert!(prompt_from_parts(&[], Some(String::new())).is_err());
}

// ── exit codes ───────────────────────────────────────────────────────

#[test]
fn status_exit_codes() {
    assert_eq!(RunStatus::Completed.exit_code(), 0);
    assert_eq!(RunStatus::Failed.exit_code(), 2);
    assert_eq!(RunStatus::MaxIterations.exit_code(), 3);
    assert_eq!(RunStatus::Cancelled.exit_code(), 130);
    assert_eq!(RunStatus::Timeout.exit_code(), 124);
}

#[test]
fn status_names() {
    assert_eq!(RunStatus::Completed.as_str(), "completed");
    assert_eq!(RunStatus::Failed.as_str(), "failed");
    assert_eq!(RunStatus::MaxIterations.as_str(), "max_iterations");
    assert_eq!(RunStatus::Cancelled.as_str(), "cancelled");
    assert_eq!(RunStatus::Timeout.as_str(), "timeout");
}

// ── blocks_text ──────────────────────────────────────────────────────

#[test]
fn blocks_text_extracts_only_text() {
    let blocks = vec![
        text_block("a"),
        ContentBlock::Thinking {
            thinking: "t".into(),
            signature: None,
        },
        text_block("b"),
    ];
    assert_eq!(blocks_text(&blocks), "a\nb");
    assert_eq!(blocks_text(&[]), "");
}

// ── RunState machine ─────────────────────────────────────────────────

#[test]
fn arms_only_on_own_echo() {
    let mut s = state("my prompt");
    // Other messages / events do not arm.
    assert!(matches!(
        s.on_event(&user_message("other")),
        Step::Continue(_)
    ));
    assert!(matches!(s.on_event(&running()), Step::Continue(_)));
    assert!(!s.armed);
    assert!(matches!(
        s.on_event(&user_message("my prompt")),
        Step::Continue(_)
    ));
    assert!(s.armed);
}

#[test]
fn ignores_stopped_before_echo() {
    let mut s = state("p");
    // A Stopped from a previous in-flight run must not terminate us.
    assert!(matches!(
        s.on_event(&stopped(StopReason::Completed {
            finish_reason: None
        })),
        Step::Continue(_)
    ));
    // Nor collected text/usage from it.
    assert!(matches!(
        s.on_event(&model_end("old answer")),
        Step::Continue(_)
    ));
    assert_eq!(s.num_turns, 0);
    assert!(s.result_text.is_empty());
}

#[test]
fn collects_result_and_usage_after_echo() {
    let mut s = state("p");
    s.on_event(&user_message("p"));
    s.on_event(&model_end("intermediate"));
    s.on_event(&Event::Model(ModelEvent::TokenUsage {
        message_id: MessageId::from("m3"),
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
        context_window: 1000,
    }));
    s.on_event(&model_end("final"));
    s.on_event(&Event::Model(ModelEvent::TokenUsage {
        message_id: MessageId::from("m4"),
        prompt_tokens: 20,
        completion_tokens: 5,
        total_tokens: 25,
        context_window: 1000,
    }));
    let Step::Done(outcome) = s.on_event(&stopped(StopReason::Completed {
        finish_reason: None,
    })) else {
        panic!("expected Done");
    };
    assert_eq!(outcome.status, RunStatus::Completed);
    // Last non-empty assistant text wins.
    assert_eq!(outcome.result_text, "final");
    assert_eq!(outcome.num_turns, 2);
    assert_eq!(
        outcome.usage,
        Usage {
            prompt_tokens: 30,
            completion_tokens: 10,
            total_tokens: 40,
        }
    );
    assert_eq!(outcome.error, None);
}

#[test]
fn empty_final_text_keeps_previous() {
    let mut s = state("p");
    s.on_event(&user_message("p"));
    s.on_event(&model_end("real answer"));
    // Final turn with tool-call-only content (no text) must not erase it.
    s.on_event(&Event::Model(ModelEvent::End {
        message_id: MessageId::from("m9"),
        content: vec![],
    }));
    let Step::Done(outcome) = s.on_event(&stopped(StopReason::Completed {
        finish_reason: None,
    })) else {
        panic!("expected Done");
    };
    assert_eq!(outcome.result_text, "real answer");
}

#[test]
fn end_turn_marker_stripped_from_result_text() {
    let mut s = state("p");
    s.on_event(&user_message("p"));
    s.on_event(&model_end("收尾记录 __YOMI_END_TURN__"));
    let Step::Done(outcome) = s.on_event(&stopped(StopReason::Completed {
        finish_reason: None,
    })) else {
        panic!("expected Done");
    };
    assert_eq!(outcome.result_text, "收尾记录");
    // 中间的惰性标记不剥。
    let mut s = state("p");
    s.on_event(&user_message("p"));
    s.on_event(&model_end("__YOMI_END_TURN__ 在中间"));
    let Step::Done(outcome) = s.on_event(&stopped(StopReason::Completed {
        finish_reason: None,
    })) else {
        panic!("expected Done");
    };
    assert_eq!(outcome.result_text, "__YOMI_END_TURN__ 在中间");
}

#[test]
fn maps_stop_reasons() {
    let cases = [
        (
            StopReason::Failed {
                error: "boom".to_string(),
            },
            RunStatus::Failed,
        ),
        (
            StopReason::MaxIterations { reached: 50 },
            RunStatus::MaxIterations,
        ),
        (
            StopReason::Cancelled { operation: None },
            RunStatus::Cancelled,
        ),
    ];
    for (reason, expected) in cases {
        let mut s = state("p");
        s.on_event(&user_message("p"));
        let Step::Done(outcome) = s.on_event(&stopped(reason)) else {
            panic!("expected Done");
        };
        assert_eq!(outcome.status, expected);
    }
}

#[test]
fn failed_stop_carries_error() {
    let mut s = state("p");
    s.on_event(&user_message("p"));
    let Step::Done(outcome) = s.on_event(&stopped(StopReason::Failed {
        error: "api down".to_string(),
    })) else {
        panic!("expected Done");
    };
    assert_eq!(outcome.error.as_deref(), Some("api down"));
}

#[test]
fn non_recoverable_error_is_terminal() {
    let mut s = state("p");
    s.on_event(&user_message("p"));
    let Step::Done(outcome) = s.on_event(&Event::Agent(AgentEvent::Error {
        phase: ErrorPhase::ToolExecution,
        error: "disk gone".to_string(),
        is_recoverable: false,
    })) else {
        panic!("expected Done");
    };
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.error.as_deref(), Some("disk gone"));
}

#[test]
fn recoverable_error_is_not_terminal() {
    let mut s = state("p");
    s.on_event(&user_message("p"));
    assert!(matches!(
        s.on_event(&Event::Agent(AgentEvent::Error {
            phase: ErrorPhase::Streaming,
            error: "flaky".to_string(),
            is_recoverable: true,
        })),
        Step::Continue(_)
    ));
}

#[test]
fn permission_request_denied_unless_yolo() {
    let req = |id: &str| {
        Event::Agent(AgentEvent::PermissionRequest {
            req_id: id.to_string(),
            session_id: "s".to_string(),
            tool_id: "t".to_string(),
            tool_name: "shell".to_string(),
            tool_args: "rm -rf /".to_string(),
            tool_level: "dangerous".to_string(),
            reason: "r".to_string(),
        })
    };

    let mut s = state("p");
    let Step::Continue(effects) = s.on_event(&req("r1")) else {
        panic!("expected Continue");
    };
    assert_eq!(
        effects,
        vec![Effect::RespondPermission {
            req_id: "r1".to_string(),
            approved: false,
        }]
    );

    let mut s = RunState::new("p".to_string(), true);
    let Step::Continue(effects) = s.on_event(&req("r2")) else {
        panic!("expected Continue");
    };
    assert_eq!(
        effects,
        vec![Effect::RespondPermission {
            req_id: "r2".to_string(),
            approved: true,
        }]
    );
}

#[test]
fn ask_user_answered_empty() {
    let mut s = state("p");
    let Step::Continue(effects) = s.on_event(&Event::Agent(AgentEvent::AskUserQuestion {
        req_id: "q1".to_string(),
        session_id: "s".to_string(),
        questions: vec![],
    })) else {
        panic!("expected Continue");
    };
    assert_eq!(
        effects,
        vec![Effect::RespondAskUser {
            req_id: "q1".to_string()
        }]
    );
}

#[test]
fn unrelated_events_ignored() {
    let mut s = state("p");
    s.on_event(&user_message("p"));
    assert!(matches!(
        s.on_event(&Event::Model(ModelEvent::Chunk {
            message_id: MessageId::from("m5"),
            content: ContentChunk::Text("chunk".to_string()),
        })),
        Step::Continue(_)
    ));
    assert!(matches!(s.on_event(&running()), Step::Continue(_)));
}

// ── JSON output ──────────────────────────────────────────────────────

#[test]
fn outcome_json_shape() {
    let outcome = RunOutcome {
        status: RunStatus::Completed,
        result_text: "done".to_string(),
        num_turns: 2,
        usage: Usage {
            prompt_tokens: 10,
            completion_tokens: 4,
            total_tokens: 14,
        },
        error: None,
    };
    let json = outcome.to_json("sess_1", Some("claude-sonnet-4"), 1234);
    assert_eq!(json["session_id"], "sess_1");
    assert_eq!(json["status"], "completed");
    assert_eq!(json["result"], "done");
    assert_eq!(json["model"], "claude-sonnet-4");
    assert_eq!(json["num_turns"], 2);
    assert_eq!(json["duration_ms"], 1234);
    assert_eq!(json["usage"]["prompt_tokens"], 10);
    assert_eq!(json["usage"]["total_tokens"], 14);
    assert!(json["error"].is_null());
}

#[test]
fn outcome_json_error() {
    let outcome = RunOutcome {
        status: RunStatus::Failed,
        result_text: String::new(),
        num_turns: 0,
        usage: Usage::default(),
        error: Some("boom".to_string()),
    };
    let json = outcome.to_json("sess_2", None, 5);
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"], "boom");
    assert!(json["model"].is_null());
}
