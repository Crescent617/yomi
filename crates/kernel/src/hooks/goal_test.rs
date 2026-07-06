use super::*;

use crate::goal::{GoalState, JsonGoalStore};
use tempfile::TempDir;

#[tokio::test]
async fn test_goal_active_returns_continue() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    let mut state = GoalState::new("do something");
    state.status = GoalStatus::Active;
    store.save("sess-1", &state).await.unwrap();

    let handler = GoalPreStopHandler::new(store);
    let ctx = crate::hooks::HookContext::pre_stop("sess-1", tmp.path());
    let result = handler.run(&ctx).await.unwrap();
    match result {
        HookResult::PreStop(d) => {
            assert!(d.continue_session);
            assert!(d.steer_blocks.is_some());
        }
        _ => panic!("expected PreStop"),
    }
}

#[tokio::test]
async fn test_no_goal_returns_passthrough() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    let handler = GoalPreStopHandler::new(store);
    let ctx = crate::hooks::HookContext::pre_stop("sess-1", tmp.path());
    let result = handler.run(&ctx).await.unwrap();
    assert!(matches!(result, HookResult::Passthrough));
}

#[tokio::test]
async fn test_completed_goal_returns_passthrough() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    let mut state = GoalState::new("do something");
    state.status = GoalStatus::Completed;
    store.save("sess-1", &state).await.unwrap();

    let handler = GoalPreStopHandler::new(store);
    let ctx = crate::hooks::HookContext::pre_stop("sess-1", tmp.path());
    let result = handler.run(&ctx).await.unwrap();
    assert!(matches!(result, HookResult::Passthrough));
}
