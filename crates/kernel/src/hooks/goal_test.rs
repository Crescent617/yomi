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

    let handler = GoalPreStopHandler::new(store, Arc::new(Default::default()));
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
async fn test_active_goal_waits_for_running_background_task() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    store
        .save("sess-1", &GoalState::new("do something"))
        .await
        .unwrap();
    let tracker = Arc::new(crate::agent::BgTaskTracker::default());
    let _guard = tracker.start(crate::types::SessionId::from("sess-1"));
    let handler = GoalPreStopHandler::new(store, tracker);

    let result = handler
        .run(&crate::hooks::HookContext::pre_stop("sess-1", tmp.path()))
        .await
        .unwrap();

    assert!(matches!(result, HookResult::Passthrough));
}

#[tokio::test]
async fn test_no_goal_returns_passthrough() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    let handler = GoalPreStopHandler::new(store, Arc::new(Default::default()));
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

    let handler = GoalPreStopHandler::new(store, Arc::new(Default::default()));
    let ctx = crate::hooks::HookContext::pre_stop("sess-1", tmp.path());
    let result = handler.run(&ctx).await.unwrap();
    assert!(matches!(result, HookResult::Passthrough));
}
