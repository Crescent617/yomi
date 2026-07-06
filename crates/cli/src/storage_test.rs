use super::*;

use tempfile::TempDir;

#[tokio::test]
async fn test_session_save_and_load() {
    let temp_dir = TempDir::new().unwrap();
    let storage = AppStorage::new(temp_dir.path()).unwrap();

    let working_dir = PathBuf::from("/path/to/project");

    assert!(storage.load_session(&working_dir).await.unwrap().is_none());
    storage
        .save_session(&working_dir, "session-123")
        .await
        .unwrap();

    let entry = storage.load_session(&working_dir).await.unwrap().unwrap();
    assert_eq!(entry.session_id, "session-123");
}

#[tokio::test]
async fn test_input_history() {
    let temp_dir = TempDir::new().unwrap();
    let storage = AppStorage::new(temp_dir.path()).unwrap();

    let working_dir = PathBuf::from("/path/to/project");

    let history = storage.load_input_history(&working_dir).await.unwrap();
    assert!(history.is_empty());

    storage
        .save_input_history(&working_dir, &["hello".to_string(), "world".to_string()])
        .await
        .unwrap();

    let history = storage.load_input_history(&working_dir).await.unwrap();
    assert_eq!(history, vec!["hello", "world"]);
}

#[tokio::test]
async fn test_input_history_dedup() {
    let temp_dir = TempDir::new().unwrap();
    let storage = AppStorage::new(temp_dir.path()).unwrap();

    let working_dir = PathBuf::from("/path/to/project");

    storage
        .save_input_history(&working_dir, &["a".to_string(), "b".to_string()])
        .await
        .unwrap();

    // Re-add "a" — should keep the latest occurrence
    storage
        .save_input_history(&working_dir, &["a".to_string()])
        .await
        .unwrap();

    let history = storage.load_input_history(&working_dir).await.unwrap();
    assert_eq!(history, vec!["b", "a"]);
}

#[tokio::test]
async fn test_input_history_empty_noop() {
    let temp_dir = TempDir::new().unwrap();
    let storage = AppStorage::new(temp_dir.path()).unwrap();

    let working_dir = PathBuf::from("/path/to/project");

    // Empty entries should not create a file
    storage.save_input_history(&working_dir, &[]).await.unwrap();

    assert!(!storage.input_hist_path(&working_dir).exists());
}

#[tokio::test]
async fn test_input_history_trim() {
    let temp_dir = TempDir::new().unwrap();
    let storage = AppStorage::new(temp_dir.path()).unwrap();

    let working_dir = PathBuf::from("/path/to/project");
    let limit = tui::INPUT_HISTORY_LIMIT;

    // Seed with limit + 1 entries → triggers trim to limit / 2
    let existing: Vec<String> = (0..=limit).map(|i| format!("old_{i}")).collect();
    storage
        .save_input_history(&working_dir, &existing)
        .await
        .unwrap();

    let history = storage.load_input_history(&working_dir).await.unwrap();
    assert_eq!(history.len(), limit / 2);
    assert_eq!(history.last().unwrap(), "old_2000");
}
