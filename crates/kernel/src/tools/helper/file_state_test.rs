use super::*;

#[tokio::test]
async fn test_file_state_store() {
    let store = FileStateStore::new();
    let path = PathBuf::from("/tmp/test.txt");

    assert!(!store.has_recorded(&path));
    assert!(store.get_mtime(&path).is_none());

    store.record(path.clone(), 12345).await;

    assert!(store.has_recorded(&path));
    assert_eq!(store.get_mtime(&path), Some(12345));
    assert!(!store.is_stale(&path, 12345));
    assert!(store.is_stale(&path, 12346));

    store.remove(&path);
    assert!(!store.has_recorded(&path));
    assert!(!store.is_stale(&path, 12345)); // unrecorded files are not stale
}

#[tokio::test]
async fn test_refresh_and_refresh_if_known() {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let path = temp_file.path().to_path_buf();
    let real_mtime = get_mtime(&path).await.unwrap();

    let store = FileStateStore::new();

    // Unknown file: refresh_if_known is a no-op (must not mark unseen files as known)
    store.refresh_if_known(&path).await;
    assert!(!store.has_recorded(&path));

    // refresh() records unconditionally
    store.refresh(&path).await;
    assert_eq!(store.get_mtime(&path), Some(real_mtime));

    // Known file: refresh_if_known updates the recorded mtime
    store.record(path.clone(), 1).await;
    store.refresh_if_known(&path).await;
    assert_eq!(store.get_mtime(&path), Some(real_mtime));
}
