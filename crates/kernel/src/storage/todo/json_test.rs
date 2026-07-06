use super::*;

use tempfile::TempDir;

async fn create_test_store() -> (JsonTodoStore, TempDir) {
    let temp = TempDir::new().unwrap();
    let store = JsonTodoStore::new(temp.path());
    (store, temp)
}

#[tokio::test]
async fn test_save_and_load() {
    let (store, _temp) = create_test_store().await;

    store.save("s1", r#"{"todos":[]}"#).await.unwrap();
    let loaded = store.load("s1").await.unwrap().unwrap();

    assert_eq!(loaded, r#"{"todos":[]}"#);
}

#[tokio::test]
async fn test_load_nonexistent() {
    let (store, _temp) = create_test_store().await;

    assert!(store.load("nonexistent").await.unwrap().is_none());
}

#[tokio::test]
async fn test_clear() {
    let (store, _temp) = create_test_store().await;

    store.save("s1", "{}").await.unwrap();
    store.clear("s1").await.unwrap();

    assert!(store.load("s1").await.unwrap().is_none());
}
