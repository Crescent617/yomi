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

#[tokio::test]
async fn test_get_resolves_stored_asset_url() {
    let (store, _temp) = create_test_store();
    let data_url = "data:image/png;base64,aGVsbG8=";

    store
        .append("session-1", &[Message::user_with_image("image", data_url)])
        .await
        .unwrap();

    let persisted = tokio::fs::read_to_string(store.file_path("session-1"))
        .await
        .unwrap();
    assert!(persisted.contains("asset://"));
    assert!(!persisted.contains(data_url));

    let messages = store.get("session-1").await.unwrap();
    let image_url = messages[0]
        .content
        .iter()
        .find_map(|block| match block {
            crate::types::ContentBlock::ImageUrl { image_url } => Some(&image_url.url),
            _ => None,
        })
        .unwrap();
    assert_eq!(image_url, data_url);
}

#[tokio::test]
async fn test_get_errors_when_stored_asset_is_missing() {
    let (store, temp) = create_test_store();
    let data_url = "data:image/png;base64,aGVsbG8=";

    store
        .append("session-1", &[Message::user_with_image("image", data_url)])
        .await
        .unwrap();

    tokio::fs::remove_dir_all(temp.path().join("assets"))
        .await
        .unwrap();

    let error = store.get("session-1").await.unwrap_err();
    assert!(error.to_string().contains("failed to resolve stored asset"));
}
