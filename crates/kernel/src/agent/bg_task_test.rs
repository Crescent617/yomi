use super::*;

#[test]
fn counts_guards_per_session() {
    let tracker = Arc::new(BgTaskTracker::default());
    let session = SessionId::from("session");
    let other = SessionId::from("other");

    let first = tracker.start(session.clone());
    let second = tracker.start(session.clone());
    assert!(tracker.is_running(&session));
    assert!(!tracker.is_running(&other));

    drop(first);
    assert!(tracker.is_running(&session));
    drop(second);
    assert!(!tracker.is_running(&session));
}

#[test]
fn is_shared_across_task_types() {
    let tracker = Arc::new(BgTaskTracker::default());
    let session = SessionId::from("session");

    let shell = tracker.start(session.clone());
    let subagent = tracker.start(session.clone());
    drop(shell);
    assert!(tracker.is_running(&session));
    drop(subagent);
    assert!(!tracker.is_running(&session));
}
