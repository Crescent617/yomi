use super::*;

use crate::types::Message;
use tempfile::TempDir;

fn create_test_store() -> (JsonlMessageStore, TempDir) {
    let temp = TempDir::new().unwrap();
    let store = JsonlMessageStore::new(temp.path(), temp.path());
    (store, temp)
}

#[tokio::test]
async fn test_append_and_get() {
    let (store, _temp) = create_test_store();

    store
        .append(
            "session-1",
            &[Message::user("hello"), Message::assistant("hi")],
        )
        .await
        .unwrap();

    let messages = store.get("session-1").await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text_content(), "hello");
    assert_eq!(messages[1].text_content(), "hi");
}

#[tokio::test]
async fn test_get_nonexistent() {
    let (store, _temp) = create_test_store();

    let messages = store.get("nonexistent").await.unwrap();
    assert!(messages.is_empty());
}

#[tokio::test]
async fn test_replace() {
    let (store, _temp) = create_test_store();

    store
        .append(
            "session-1",
            &[Message::user("old"), Message::assistant("data")],
        )
        .await
        .unwrap();

    store
        .replace("session-1", &[Message::user("compacted")])
        .await
        .unwrap();

    let messages = store.get("session-1").await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text_content(), "compacted");
}
