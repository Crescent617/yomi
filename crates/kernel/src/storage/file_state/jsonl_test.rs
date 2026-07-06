use super::*;

use tempfile::TempDir;

fn create_test_store() -> (JsonlFileStateStore, TempDir) {
    let temp = TempDir::new().unwrap();
    let store = JsonlFileStateStore::new("test-session", temp.path());
    (store, temp)
}

#[tokio::test]
async fn test_lazy_init_no_file_until_record() {
    let temp = TempDir::new().unwrap();
    let store = JsonlFileStateStore::new("lazy-test", temp.path());

    // File should not exist before first record
    let file_path = temp.path().join("sessions/file_states/lazy-test.jsonl");
    assert!(!file_path.exists());

    // After record, file should exist
    store.record(PathBuf::from("/tmp/a.rs"), 100).await.unwrap();
    assert!(file_path.exists());
}

#[tokio::test]
async fn test_read_all_empty_when_no_file() {
    let (store, _temp) = create_test_store();

    // Should return empty without creating file
    let states = store.read_all().await.unwrap();
    assert!(states.is_empty());
}

#[tokio::test]
async fn test_truncate_no_op_when_no_file() {
    let (store, _temp) = create_test_store();

    // Should be no-op without creating file
    store.truncate().await.unwrap();
}

#[tokio::test]
async fn test_record_and_get() {
    let (store, _temp) = create_test_store();

    store.record(PathBuf::from("/tmp/a.rs"), 100).await.unwrap();
    store.record(PathBuf::from("/tmp/b.rs"), 200).await.unwrap();

    let states = store.read_all().await.unwrap();
    assert_eq!(states.len(), 2);
}

#[tokio::test]
async fn test_duplicate_paths_keep_latest() {
    let (store, _temp) = create_test_store();

    store
        .record(PathBuf::from("/tmp/test.rs"), 100)
        .await
        .unwrap();
    store
        .record(PathBuf::from("/tmp/test.rs"), 200)
        .await
        .unwrap();

    let states = store.read_all().await.unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].path, PathBuf::from("/tmp/test.rs"));
    assert_eq!(states[0].mtime, 200);
}

#[tokio::test]
async fn test_clear() {
    let (store, _temp) = create_test_store();

    store
        .record(PathBuf::from("/tmp/test.rs"), 100)
        .await
        .unwrap();
    store.truncate().await.unwrap();

    let states = store.read_all().await.unwrap();
    assert!(states.is_empty());
}

#[tokio::test]
async fn test_persist_across_reopen() {
    let temp = TempDir::new().unwrap();
    let session_id = "persist-session";

    {
        let store = JsonlFileStateStore::new(session_id, temp.path());
        store
            .record(PathBuf::from("/tmp/test.rs"), 123)
            .await
            .unwrap();
    }

    {
        let store = JsonlFileStateStore::new(session_id, temp.path());
        let states = store.read_all().await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].path, PathBuf::from("/tmp/test.rs"));
        assert_eq!(states[0].mtime, 123);
    }
}

#[tokio::test]
async fn test_auto_vacuum() {
    let temp = TempDir::new().unwrap();

    // Create store with low threshold
    let store = JsonlFileStateStore::new("vacuum-test", temp.path());

    // Write 1000+ records to trigger auto-vacuum
    for i in 0..1005 {
        store
            .record(PathBuf::from("/tmp/same.rs"), 100 + i as u64)
            .await
            .unwrap();
    }

    // After vacuum, only 1 unique path remains
    let states = store.read_all().await.unwrap();
    assert_eq!(states.len(), 1);
    // Latest mtime is 100 + 1004 = 1104
    assert_eq!(states[0].path, PathBuf::from("/tmp/same.rs"));
    assert_eq!(states[0].mtime, 1104);
}
