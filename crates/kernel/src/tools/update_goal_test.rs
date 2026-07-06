use super::*;

use crate::goal::{GoalState, JsonGoalStore};
use tempfile::TempDir;

#[tokio::test]
async fn test_update_goal_completed() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    let mut state = GoalState::new("do something");
    state.status = GoalStatus::Active;
    store.save("sess-1", &state).await.unwrap();

    let tool = UpdateGoalTool::new(store.clone());
    let ctx = ToolExecCtx::new("tc-1", tmp.path(), "sess-1");
    let result = tool
        .exec(json!({"status": "completed"}), ctx)
        .await
        .unwrap();
    let text = result.contents[0].as_text().unwrap();
    assert!(text.contains("completed"));

    let loaded = store.load("sess-1").await.unwrap().unwrap();
    assert!(matches!(loaded.status, GoalStatus::Completed));
}

#[tokio::test]
async fn test_update_goal_blocked() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    let mut state = GoalState::new("do something");
    state.status = GoalStatus::Active;
    store.save("sess-1", &state).await.unwrap();

    let tool = UpdateGoalTool::new(store.clone());
    let ctx = ToolExecCtx::new("tc-1", tmp.path(), "sess-1");
    let result = tool.exec(json!({"status": "blocked"}), ctx).await.unwrap();
    let text = result.contents[0].as_text().unwrap();
    assert!(text.contains("blocked"));

    let loaded = store.load("sess-1").await.unwrap().unwrap();
    assert!(matches!(loaded.status, GoalStatus::Blocked));
}

#[tokio::test]
async fn test_update_goal_no_goal() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    let tool = UpdateGoalTool::new(store);
    let ctx = ToolExecCtx::new("tc-1", tmp.path(), "sess-1");
    let result = tool.exec(json!({"status": "completed"}), ctx).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_update_goal_invalid_status() {
    let tmp = TempDir::new().unwrap();
    let store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(tmp.path()));
    let mut state = GoalState::new("do something");
    state.status = GoalStatus::Active;
    store.save("sess-1", &state).await.unwrap();

    let tool = UpdateGoalTool::new(store);
    let ctx = ToolExecCtx::new("tc-1", tmp.path(), "sess-1");
    let result = tool.exec(json!({"status": "invalid"}), ctx).await;
    assert!(result.is_err());
}
