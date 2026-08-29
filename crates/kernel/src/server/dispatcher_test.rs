//! Wire round-trip test: exercises the favorite methods through
//! `RemoteKernel` (IPC client) → transport → dispatcher → `Kernel`,
//! the same path the GUI uses.

use crate::client::{KernelApi, RemoteKernel};
use crate::config::Config;
use crate::storage::AddFavoriteInput;
use crate::types::{MessageId, SessionId};
use tempfile::TempDir;

async fn setup_with_config_path(
    config_path: Option<std::path::PathBuf>,
    restart_tx: Option<tokio::sync::mpsc::Sender<()>>,
) -> (RemoteKernel, TempDir, tokio_util::sync::CancellationToken) {
    let tmp = TempDir::new().unwrap();
    let mut config = Config {
        data_dir: tmp.path().to_path_buf(),
        ..Config::default()
    };
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();
    let server = crate::server::KernelServer::with_lifecycle(kernel, config_path, restart_tx);
    server.start(&config).await;
    let addr = crate::transport::SocketAddr::Unix(tmp.path().join("daemon.sock"));
    let listener = crate::transport::bind(&addr, None).await.unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let serve_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = server.serve(listener, serve_shutdown).await;
    });
    let client = RemoteKernel::connect(&addr).await.unwrap();
    (client, tmp, shutdown)
}

async fn setup() -> (RemoteKernel, TempDir, tokio_util::sync::CancellationToken) {
    setup_with_config_path(None, None).await
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

#[tokio::test]
async fn test_config_wire_round_trip() {
    let root = TempDir::new().unwrap();
    let config_path = root.path().join("config.toml");
    let original = "max_checkpoints = 7\n";
    std::fs::write(&config_path, original).unwrap();
    let (client, _socket_dir, shutdown) =
        setup_with_config_path(Some(config_path.clone()), None).await;

    let config = client.get_config().await.unwrap();
    assert_eq!(config.content, original);
    assert_eq!(config.path, config_path.to_string_lossy());
    assert!(config.full_config.contains("max_checkpoints = 7"));

    let updated = "max_checkpoints = 9\n";
    client.set_config(updated.to_string()).await.unwrap();
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), updated);
    assert_eq!(client.get_config().await.unwrap().content, updated);

    let error = client
        .set_config("auto_approve = \"unsupported\"\n".to_string())
        .await
        .expect_err("invalid config should fail");
    assert!(error.to_string().contains("Invalid TOML"));
    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), updated);

    shutdown.cancel();
}

#[tokio::test]
async fn test_restart_wire_request() {
    let (restart_tx, mut restart_rx) = tokio::sync::mpsc::channel(1);
    let (client, _tmp, shutdown) = setup_with_config_path(None, Some(restart_tx)).await;

    let call = tokio::spawn(async move { client.restart().await });
    tokio::time::timeout(std::time::Duration::from_secs(1), restart_rx.recv())
        .await
        .expect("restart request timeout")
        .expect("restart channel closed");
    shutdown.cancel();
    let result = call.await.unwrap();
    assert!(result.is_err(), "test server is intentionally not replaced");
}

#[tokio::test]
async fn test_agent_templates_wire_round_trip() {
    use crate::agent_tmpl::{TemplateScope, TemplateSource};

    let (client, tmp, shutdown) = setup().await;

    // builtin floor is visible without any session context
    let list = client.list_agent_templates(None).await.unwrap();
    assert!(list
        .iter()
        .any(|t| t.name == "planner" && t.source == TemplateSource::Builtin));

    // save global → listed with effective body, file on disk
    client
        .save_agent_template(None, TemplateScope::Global, "wire-role", "# Wire\nbody")
        .await
        .unwrap();
    let list = client.list_agent_templates(None).await.unwrap();
    let t = list.iter().find(|t| t.name == "wire-role").unwrap();
    assert_eq!(t.source, TemplateSource::Global);
    assert_eq!(t.body, "# Wire\nbody");
    assert!(tmp.path().join("agents/wire-role/ROLE.md").exists());

    // override builtin, then delete → builtin restored
    client
        .save_agent_template(None, TemplateScope::Global, "planner", "custom")
        .await
        .unwrap();
    let list = client.list_agent_templates(None).await.unwrap();
    assert_eq!(
        list.iter().find(|t| t.name == "planner").unwrap().source,
        TemplateSource::Global
    );
    client
        .delete_agent_template(None, TemplateScope::Global, "planner")
        .await
        .unwrap();
    let list = client.list_agent_templates(None).await.unwrap();
    assert_eq!(
        list.iter().find(|t| t.name == "planner").unwrap().source,
        TemplateSource::Builtin
    );

    // invalid name rejected; workspace scope needs a session context
    assert!(client
        .save_agent_template(None, TemplateScope::Global, "../bad", "x")
        .await
        .is_err());
    assert!(client
        .save_agent_template(None, TemplateScope::Workspace, "ws-role", "x")
        .await
        .is_err());

    shutdown.cancel();
}

// ── Socket auth over ws (RemoteKernel token propagation) ────────────────

/// This host's primary non-loopback IPv4, if any: auth only gates remote
/// peers, so the rejection path needs a dial whose source isn't loopback
/// (connecting to one's own NIC address keeps it as the peer source).
fn non_loopback_host() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback()).then(|| ip.to_string())
}

async fn setup_ws_auth(
    password: &str,
) -> Option<(
    crate::transport::SocketAddr,
    TempDir,
    tokio_util::sync::CancellationToken,
)> {
    let host = non_loopback_host()?;
    let tmp = TempDir::new().unwrap();
    let mut config = Config {
        data_dir: tmp.path().to_path_buf(),
        ..Config::default()
    };
    config.finalize();
    let kernel = crate::build_kernel(&config, false).await.unwrap();
    let server = crate::server::KernelServer::with_lifecycle(kernel, None, None);
    server.start(&config).await;
    let auth = Some(crate::transport::auth_verifier(
        &crate::transport::hash_password(password),
    ));
    let listener =
        crate::transport::bind(&crate::transport::SocketAddr::Ws("0.0.0.0:0".into()), auth)
            .await
            .unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = crate::transport::SocketAddr::Ws(format!("{host}:{port}"));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let serve_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = server.serve(listener, serve_shutdown).await;
    });
    Some((addr, tmp, shutdown))
}

#[tokio::test]
async fn test_remote_kernel_connect_with_auth() {
    let Some((addr, _tmp, shutdown)) = setup_ws_auth("pw-123").await else {
        eprintln!("skip: host has no non-loopback interface");
        return;
    };

    // Correct token: handshake + Hello succeed, kernel is usable. A
    // fake-IP VPN can hijack the dial (connection dies pre-handshake) —
    // skip then; an auth-flavored error means the server was reached yet
    // rejected a good token: fail.
    let client = match RemoteKernel::connect_with_auth(&addr, Some("pw-123".to_string())).await {
        Ok(client) => client,
        Err(e) => {
            shutdown.cancel();
            assert!(
                !e.to_string().contains("socket auth failed"),
                "correct token rejected: {e}"
            );
            eprintln!("skip: non-loopback dial unusable ({e})");
            return;
        }
    };
    client.check_ready().await.unwrap();

    // Wrong / missing tokens are rejected during the ws handshake — but
    // only when the dial presents a remote peer. macOS hairpins
    // self-dials to lo0, so the peer is loopback and the (correct)
    // bypass lets the connection through: that environment simply can't
    // exercise rejection, skip it there. When rejection does happen it
    // must carry the socket-auth error.
    for token in [Some("nope".to_string()), None] {
        match RemoteKernel::connect_with_auth(&addr, token).await {
            Err(e) => assert!(
                e.to_string().contains("socket auth failed"),
                "unexpected error: {e}"
            ),
            Ok(_) => {
                eprintln!("skip: self-dial hairpins to loopback; rejection untestable here");
                break;
            }
        }
    }

    shutdown.cancel();
}

#[tokio::test]
async fn test_set_channel_watch_without_channels() {
    let (client, _tmp, shutdown) = setup().await;
    let err = client
        .set_channel_watch(None, None, "oc_x".to_string(), None)
        .await
        .err()
        .expect("no channels configured must error");
    assert!(
        err.to_string().contains("no channels are running"),
        "unexpected error: {err}"
    );
    shutdown.cancel();
}
