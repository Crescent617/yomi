use super::*;

#[test]
fn counts_guards_per_session() {
    let tracker = Arc::new(BgTaskTracker::default());
    let session = SessionId::from("session");
    let other = SessionId::from("other");

    let first = tracker.start(session.clone(), BackgroundTaskKind::Shell);
    let second = tracker.start(session.clone(), BackgroundTaskKind::Shell);
    assert_eq!(tracker.count(&session, BackgroundTaskKind::Shell), 2);
    assert_eq!(tracker.active_session_ids(), vec![session.clone()]);
    assert!(tracker.is_running(&session));
    assert!(!tracker.is_running(&other));

    drop(first);
    assert!(tracker.is_running(&session));
    drop(second);
    assert!(!tracker.is_running(&session));
    assert!(tracker.active_session_ids().is_empty());
    assert_eq!(tracker.count(&session, BackgroundTaskKind::Subagent), 0);
}

#[test]
fn is_shared_across_task_types() {
    let tracker = Arc::new(BgTaskTracker::default());
    let session = SessionId::from("session");

    let shell = tracker.start(session.clone(), BackgroundTaskKind::Shell);
    let subagent = tracker.start(session.clone(), BackgroundTaskKind::Subagent);
    assert_eq!(tracker.count(&session, BackgroundTaskKind::Shell), 1);
    assert_eq!(tracker.count(&session, BackgroundTaskKind::Subagent), 1);
    drop(shell);
    assert!(tracker.is_running(&session));
    assert_eq!(tracker.count(&session, BackgroundTaskKind::Shell), 0);
    drop(subagent);
    assert!(!tracker.is_running(&session));
    assert_eq!(tracker.count(&session, BackgroundTaskKind::Subagent), 0);
}

#[test]
fn shell_task_details_are_removed_with_guard() {
    let tracker = Arc::new(BgTaskTracker::default());
    let session = SessionId::from("session-shell");
    let task = BackgroundShellTask {
        task_id: "sh-1".to_string(),
        session_id: session.clone(),
        pid: 42,
        command: "cargo test".to_string(),
        output_path: "/tmp/sh-1.log".to_string(),
        started_at: chrono::Utc::now(),
    };

    let guard = tracker.start_shell(task.clone());
    assert_eq!(tracker.shell_tasks(), vec![task]);
    assert_eq!(tracker.count(&session, BackgroundTaskKind::Shell), 1);

    drop(guard);
    assert!(tracker.shell_tasks().is_empty());
    assert_eq!(tracker.count(&session, BackgroundTaskKind::Shell), 0);
}
