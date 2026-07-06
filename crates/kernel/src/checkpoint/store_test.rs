use super::*;

use crate::checkpoint::CheckpointStore as _;
use tempfile::TempDir;

fn create_test_store() -> (FilesystemCheckpointStore, TempDir) {
    let temp = TempDir::new().unwrap();
    let store = FilesystemCheckpointStore::new(temp.path());
    (store, temp)
}

#[tokio::test]
async fn test_create_and_list_checkpoints() {
    let (store, _temp) = create_test_store();

    // Create checkpoints
    let cp1 = store
        .create_checkpoint("session-1", "msg-1", "Hello world", vec![])
        .await
        .unwrap();
    assert_eq!(cp1.sequence, 1);

    let cp2 = store
        .create_checkpoint("session-1", "msg-2", "How are you", vec![])
        .await
        .unwrap();
    assert_eq!(cp2.sequence, 2);

    // List checkpoints
    let checkpoints = store.get_session_checkpoints("session-1").await.unwrap();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(checkpoints[0].sequence, 1);
    assert_eq!(checkpoints[1].sequence, 2);
}

#[tokio::test]
async fn test_delete_session_checkpoints() {
    let (store, _temp) = create_test_store();

    store
        .create_checkpoint("session-1", "msg-1", "Test", vec![])
        .await
        .unwrap();
    store
        .create_checkpoint("session-1", "msg-2", "Test", vec![])
        .await
        .unwrap();

    let deleted = store.delete_session_checkpoints("session-1").await.unwrap();
    assert_eq!(deleted, 2);

    let checkpoints = store.get_session_checkpoints("session-1").await.unwrap();
    assert!(checkpoints.is_empty());
}

#[tokio::test]
async fn test_rewind_creates_messages_backup() {
    use std::io::Write;

    let (store, temp) = create_test_store();

    // Create sessions directory and a messages file
    let sessions_dir = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let messages_path = sessions_dir.join("session-1.jsonl");
    {
        let mut file = std::fs::File::create(&messages_path).unwrap();
        writeln!(file, r#"{{"id":"msg-1","role":"user","content":"hello"}}"#).unwrap();
    }

    // Create a checkpoint
    store
        .create_checkpoint("session-1", "msg-1", "Hello", vec![])
        .await
        .unwrap();

    // Modify messages file
    {
        let mut file = std::fs::File::create(&messages_path).unwrap();
        writeln!(file, r#"{{"id":"msg-1","role":"user","content":"hello"}}"#).unwrap();
        writeln!(
            file,
            r#"{{"id":"msg-2","role":"assistant","content":"hi"}}"#
        )
        .unwrap();
    }

    // Create second checkpoint
    store
        .create_checkpoint("session-1", "msg-2", "Hi there", vec![])
        .await
        .unwrap();

    // Verify we have 2 checkpoints
    let checkpoints = store.get_session_checkpoints("session-1").await.unwrap();
    assert_eq!(checkpoints.len(), 2);

    // Rewind to first checkpoint (destructive - deletes target and all after)
    store.rewind_to_checkpoint("session-1", 1).await.unwrap();

    // Verify no checkpoints remain (target was deleted)
    let checkpoints = store.get_session_checkpoints("session-1").await.unwrap();
    assert_eq!(checkpoints.len(), 0);

    // Verify messages file was restored
    let content = std::fs::read_to_string(&messages_path).unwrap();
    assert!(content.contains("msg-1"));
    assert!(!content.contains("msg-2"));
}
