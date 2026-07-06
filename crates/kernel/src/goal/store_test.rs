use super::*;

use tempfile::TempDir;

#[tokio::test]
async fn test_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let store = JsonGoalStore::new(tmp.path());

    let state = GoalState::new("do something");
    store.save("sess-1", &state).await.unwrap();

    let loaded = store.load("sess-1").await.unwrap().unwrap();
    assert_eq!(loaded.description, "do something");
}

#[tokio::test]
async fn test_load_missing() {
    let tmp = TempDir::new().unwrap();
    let store = JsonGoalStore::new(tmp.path());
    assert!(store.load("nonexistent").await.unwrap().is_none());
}

#[tokio::test]
async fn test_delete() {
    let tmp = TempDir::new().unwrap();
    let store = JsonGoalStore::new(tmp.path());

    let state = GoalState::new("x");
    store.save("s", &state).await.unwrap();
    assert!(store.load("s").await.unwrap().is_some());

    store.delete("s").await.unwrap();
    assert!(store.load("s").await.unwrap().is_none());
}
