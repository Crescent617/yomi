use std::time::{Duration, Instant};

use kernel::event::StopReason;
use kernel::notification::AgentActivity;

use crate::pet::{
    PetMood, PetNoticeKind, PetRequest, PetRuntime, PET_IDLE_TIMEOUT, PET_NOTICE_DURATION,
};

#[test]
fn idle_becomes_sleepy_after_timeout_without_running_sessions() {
    let started_at = Instant::now();
    let runtime = PetRuntime::new(started_at);

    assert_eq!(runtime.snapshot(started_at).mood, PetMood::Idle);
    assert_eq!(
        runtime
            .snapshot(
                (started_at + PET_IDLE_TIMEOUT)
                    .checked_sub(Duration::from_millis(1))
                    .unwrap(),
            )
            .mood,
        PetMood::Idle
    );
    assert_eq!(
        runtime.snapshot(started_at + PET_IDLE_TIMEOUT).mood,
        PetMood::Sleepy
    );
}

#[test]
fn running_session_prevents_sleep() {
    let started_at = Instant::now();
    let mut runtime = PetRuntime::new(started_at);
    runtime.set_session("session-1", Some("Build pet"), true);

    assert_eq!(
        runtime.snapshot(started_at + PET_IDLE_TIMEOUT * 2).mood,
        PetMood::Working
    );
}

#[test]
fn notification_activity_wakes_sleepy_runtime() {
    let started_at = Instant::now();
    let mut runtime = PetRuntime::new(started_at);
    let activity_at = started_at + PET_IDLE_TIMEOUT;

    assert_eq!(runtime.snapshot(activity_at).mood, PetMood::Sleepy);
    runtime.record_activity(activity_at);
    assert_eq!(runtime.snapshot(activity_at).mood, PetMood::Idle);
}

#[test]
fn completed_notice_is_happy_then_returns_to_idle() {
    let started_at = Instant::now();
    let mut runtime = PetRuntime::new(started_at);
    runtime.set_session("session-1", Some("Build pet"), true);

    assert!(runtime.process_activity(
        "session-1",
        "event-1",
        &AgentActivity::Stopped {
            reason: StopReason::Completed {
                finish_reason: None,
            },
        },
        started_at,
    ));
    let active = runtime.snapshot(started_at);
    assert_eq!(active.mood, PetMood::Happy);
    assert_eq!(active.running_count, 0);
    assert_eq!(active.notice.unwrap().kind, PetNoticeKind::Completed);

    let expired_at = started_at + PET_NOTICE_DURATION;
    assert!(runtime.expire(expired_at));
    let expired = runtime.snapshot(expired_at);
    assert_eq!(expired.mood, PetMood::Idle);
    assert!(expired.notice.is_none());
}

#[test]
fn requests_are_added_and_removed_by_matching_ack() {
    let now = Instant::now();
    let mut runtime = PetRuntime::new(now);
    runtime.set_session("session-1", Some("Build pet"), true);

    assert!(runtime.process_activity(
        "session-1",
        "event-1",
        &AgentActivity::PermissionRequested {
            req_id: "req-1".into(),
            target_session_id: "session-1".into(),
        },
        now,
    ));
    assert_eq!(runtime.snapshot(now).mood, PetMood::Alert);

    assert!(runtime.process_activity(
        "session-1",
        "event-2",
        &AgentActivity::RequestResolved {
            req_id: "req-1".into(),
        },
        now,
    ));
    assert_eq!(runtime.snapshot(now).mood, PetMood::Working);
}

#[test]
fn permission_request_takes_priority_over_ask_user() {
    let now = Instant::now();
    let mut runtime = PetRuntime::new(now);

    assert!(runtime.process_activity(
        "session-1",
        "event-1",
        &AgentActivity::AskUserRequested {
            req_id: "ask-1".into(),
            target_session_id: "session-1".into(),
        },
        now,
    ));
    assert!(runtime.process_activity(
        "session-1",
        "event-2",
        &AgentActivity::PermissionRequested {
            req_id: "perm-1".into(),
            target_session_id: "session-1".into(),
        },
        now,
    ));

    let snapshot = runtime.snapshot(now);
    assert_eq!(snapshot.mood, PetMood::Alert);
    assert!(matches!(
        snapshot.request,
        Some(PetRequest::Permission { req_id, .. }) if req_id == "perm-1"
    ));
}

#[test]
fn duplicate_event_does_not_duplicate_notice() {
    let now = Instant::now();
    let mut runtime = PetRuntime::new(now);
    runtime.set_session("session-1", None, true);
    let activity = AgentActivity::Stopped {
        reason: StopReason::Completed {
            finish_reason: None,
        },
    };

    assert!(runtime.process_activity("session-1", "event-1", &activity, now));
    assert!(!runtime.process_activity("session-1", "event-1", &activity, now));
    assert_eq!(runtime.snapshot(now).running_count, 0);
}
