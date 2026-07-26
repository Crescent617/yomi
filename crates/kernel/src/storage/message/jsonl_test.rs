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
async fn test_get_keeps_asset_reference() {
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

    // Frontend reads keep the lightweight asset reference (resolved lazily
    // via read_asset) so list responses stay small.
    let messages = store.get("session-1").await.unwrap();
    let image_url = messages[0]
        .content
        .iter()
        .find_map(|block| match block {
            crate::types::ContentBlock::ImageUrl { image_url } => Some(&image_url.url),
            _ => None,
        })
        .unwrap();
    assert!(image_url.starts_with("asset://"));
}

#[tokio::test]
async fn test_get_inlined_resolves_stored_asset_url() {
    let (store, _temp) = create_test_store();
    let data_url = "data:image/png;base64,aGVsbG8=";

    store
        .append("session-1", &[Message::user_with_image("image", data_url)])
        .await
        .unwrap();

    // Model-context reads inline the asset back to a data URL.
    let messages = store.get_inlined("session-1").await.unwrap();
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
async fn test_get_inlined_placeholder_when_stored_asset_is_missing() {
    let (store, temp) = create_test_store();
    let data_url = "data:image/png;base64,aGVsbG8=";

    store
        .append("session-1", &[Message::user_with_image("image", data_url)])
        .await
        .unwrap();

    tokio::fs::remove_dir_all(temp.path().join("assets"))
        .await
        .unwrap();

    // Missing assets never fail the load: get keeps the reference,
    // get_inlined degrades the block to a text placeholder.
    let messages = store.get("session-1").await.unwrap();
    assert!(messages[0].content.iter().any(|block| matches!(
        block,
        crate::types::ContentBlock::ImageUrl { image_url } if image_url.url.starts_with("asset://")
    )));

    let messages = store.get_inlined("session-1").await.unwrap();
    assert!(messages[0].content.iter().any(|block| matches!(
        block,
        crate::types::ContentBlock::Text { text } if text.contains("[image unavailable:")
    )));
}
