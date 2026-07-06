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
