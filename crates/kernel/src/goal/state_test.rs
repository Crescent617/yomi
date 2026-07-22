use super::*;

#[test]
fn test_default_state() {
    let s = GoalState::default();
    assert!(s.description.is_empty());
    assert!(matches!(s.status, GoalStatus::Active));
}

#[test]
fn test_builder() {
    let s = GoalState::new("do stuff");
    assert_eq!(s.description, "do stuff");
}

#[test]
fn test_serde_roundtrip() {
    let s = GoalState::new("test goal");
    let json = serde_json::to_string(&s).unwrap();
    let decoded: GoalState = serde_json::from_str(&json).unwrap();
    assert_eq!(s.description, decoded.description);
    assert_eq!(s.status, decoded.status);
}

#[test]
fn test_continue_prompt() {
    let state = GoalState::new("test goal");
    let p = state.build_continue_prompt();
    assert!(p.contains("Continue working toward the active goal"));
    assert!(p.contains("test goal"));
    assert!(p.contains("Completion audit"));
    assert!(p.contains("Blocked audit"));
    assert!(p.contains("update_goal"));
}
