use super::*;

use crate::types::SessionId;
use tempfile::TempDir;

#[tokio::test]
async fn test_storage_set_open() {
    let temp_dir = TempDir::new().unwrap();
    let storage = StorageSet::open(temp_dir.path()).await.unwrap();

    // Verify all stores are functional
    let session_id = SessionId::new();
    storage
        .session_store()
        .create(&session_id, None, None, None, None, None)
        .await
        .unwrap();
    storage
        .message_store()
        .append(&session_id.0, &[])
        .await
        .unwrap();
    storage
        .todo_store()
        .save(&session_id.0, "{}")
        .await
        .unwrap();

    // Verify data directory structure
    assert!(temp_dir.path().join("yomi.db").exists());
    assert!(temp_dir.path().join("sessions").exists());
    assert!(temp_dir.path().join("sessions/todos").exists());
}

#[tokio::test]
async fn test_file_state_store() {
    let temp_dir = TempDir::new().unwrap();
    let storage = StorageSet::open(temp_dir.path()).await.unwrap();

    let file_store = storage.file_state_store("test-session");
    // Just verify it was created successfully
    drop(file_store);
}
