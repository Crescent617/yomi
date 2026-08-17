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

/// 在指定 checkpoint 目录写入一个文件备份对象（objects/<hash前两位>/<hash>）。
fn write_backup_object(temp: &TempDir, session_id: &str, msg_id: &str, hash: &str, content: &str) {
    let dir = temp
        .path()
        .join("checkpoints")
        .join(session_id)
        .join(msg_id)
        .join("objects")
        .join(&hash[..2]);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(hash), content).unwrap();
}

/// 回滚到非最新 checkpoint 时，文件必须恢复到"目标时刻"的内容——即目标之后
/// 第一次修改前的备份（最旧备份胜出），而不是最后一次修改前的中间版本。
#[tokio::test]
async fn test_rewind_restores_oldest_post_target_backup() {
    use crate::checkpoint::{FileOp, TrackedFileInfo};

    let (store, temp) = create_test_store();
    let work = TempDir::new().unwrap();
    let file = work.path().join("a.txt");

    // seq1：目标 checkpoint，无跟踪文件。
    store
        .create_checkpoint("session-1", "msg-1", "target", vec![])
        .await
        .unwrap();

    // seq2：第一次修改，备份是目标时刻的内容。
    store
        .create_checkpoint(
            "session-1",
            "msg-2",
            "turn 2",
            vec![TrackedFileInfo {
                path: file.clone(),
                backup_hash: "aa00".to_string(),
                op: FileOp::Modify,
            }],
        )
        .await
        .unwrap();
    write_backup_object(&temp, "session-1", "msg-2", "aa00", "at-target");

    // seq3：第二次修改，备份是中间版本。
    store
        .create_checkpoint(
            "session-1",
            "msg-3",
            "turn 3",
            vec![TrackedFileInfo {
                path: file.clone(),
                backup_hash: "bb00".to_string(),
                op: FileOp::Modify,
            }],
        )
        .await
        .unwrap();
    write_backup_object(&temp, "session-1", "msg-3", "bb00", "intermediate");

    std::fs::write(&file, "latest").unwrap();

    store.rewind_to_checkpoint("session-1", 1).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "at-target",
        "must restore the state at target time, not an intermediate version"
    );
}

/// 目标之后"先创建后修改"的文件，回滚后必须被删除（最旧记录 Create 胜出），
/// 而不是被恢复成修改前的中间版本。
#[tokio::test]
async fn test_rewind_deletes_file_created_then_modified_after_target() {
    use crate::checkpoint::{FileOp, TrackedFileInfo};

    let (store, temp) = create_test_store();
    let work = TempDir::new().unwrap();
    let file = work.path().join("new.txt");

    store
        .create_checkpoint("session-1", "msg-1", "target", vec![])
        .await
        .unwrap();

    // seq2：文件被创建（无备份）。
    store
        .create_checkpoint(
            "session-1",
            "msg-2",
            "turn 2",
            vec![TrackedFileInfo {
                path: file.clone(),
                backup_hash: "NULL".to_string(),
                op: FileOp::Create,
            }],
        )
        .await
        .unwrap();

    // seq3：同一文件被修改，备份是创建时的中间版本。
    store
        .create_checkpoint(
            "session-1",
            "msg-3",
            "turn 3",
            vec![TrackedFileInfo {
                path: file.clone(),
                backup_hash: "cc00".to_string(),
                op: FileOp::Modify,
            }],
        )
        .await
        .unwrap();
    write_backup_object(&temp, "session-1", "msg-3", "cc00", "intermediate");

    std::fs::write(&file, "latest").unwrap();

    store.rewind_to_checkpoint("session-1", 1).await.unwrap();

    assert!(
        !file.exists(),
        "file created after target must be deleted, not restored to an intermediate version"
    );
}
