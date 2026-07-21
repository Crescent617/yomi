//! Wire round-trip test: exercises the favorite methods through
//! `RemoteKernel` (IPC client) → transport → dispatcher → `Kernel`,
//! the same path the GUI uses.

use crate::client::{KernelApi, RemoteKernel};
use crate::config::Config;
use crate::storage::AddFavoriteInput;
use crate::types::{MessageId, SessionId};
use tempfile::TempDir;

async fn setup() -> (RemoteKernel, TempDir, tokio_util::sync::CancellationToken) {
    let tmp = TempDir::new().unwrap();
    let mut config = Config {
        data_dir: tmp.path().to_path_buf(),
        ..Config::default()
    };
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();
    let server = crate::server::KernelServer::new(kernel);
    server.start(Vec::new()).await;
    let addr = crate::transport::SocketAddr::Unix(tmp.path().join("daemon.sock"));
    let listener = crate::transport::bind(&addr).await.unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let serve_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = server.serve(listener, serve_shutdown).await;
    });
    let client = RemoteKernel::connect(&addr).await.unwrap();
    (client, tmp, shutdown)
}

fn make_input() -> AddFavoriteInput {
    AddFavoriteInput {
        session_id: SessionId::new(),
        message_id: MessageId::new(),
        session_title: Some("Wire test".to_string()),
        content: "hello **wire**".to_string(),
        note: None,
        message_created_at: None,
    }
}

#[tokio::test]
async fn test_favorites_wire_round_trip() {
    let (client, _tmp, shutdown) = setup().await;
    let input = make_input();

    // add
    let added = client.add_favorite(input.clone()).await.unwrap();
    assert!(added.id.starts_with("fav_"));
    assert_eq!(added.content, "hello **wire**");
    assert_eq!(added.session_title.as_deref(), Some("Wire test"));

    // list + search
    let list = client.list_favorites(None, 10, 0).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, added.id);
    let search = client
        .list_favorites(Some("wire".to_string()), 10, 0)
        .await
        .unwrap();
    assert_eq!(search.len(), 1);

    // update note
    client
        .update_favorite_note(&added.id, Some("noted".to_string()))
        .await
        .unwrap();
    let list = client.list_favorites(None, 10, 0).await.unwrap();
    assert_eq!(list[0].note.as_deref(), Some("noted"));

    // remove by message
    client
        .remove_favorite_by_message(&input.session_id, &input.message_id)
        .await
        .unwrap();
    assert!(client.list_favorites(None, 10, 0).await.unwrap().is_empty());

    // remove by id
    let added = client.add_favorite(input).await.unwrap();
    client.remove_favorite(&added.id).await.unwrap();
    assert!(client.list_favorites(None, 10, 0).await.unwrap().is_empty());

    shutdown.cancel();
}
