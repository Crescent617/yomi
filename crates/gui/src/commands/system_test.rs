use super::*;

#[test]
fn connection_info_uses_snake_case_fields_for_local_daemon() {
    let info = connection_info_json(&crate::state::ConnectionMode::Local, true);

    assert_eq!(info["mode"], "local");
    assert_eq!(info["addr"], crate::daemon::socket_addr().to_string());
    assert_eq!(info["managed"], true);
}

#[test]
fn connection_info_reports_remote_daemon() {
    let addr = kernel::transport::SocketAddr::Wss("example.com/kernel".to_string());
    let info = connection_info_json(&crate::state::ConnectionMode::Remote(addr), false);

    assert_eq!(info["mode"], "remote");
    assert_eq!(info["addr"], "wss://example.com/kernel");
    assert_eq!(info["managed"], false);
}

#[test]
fn remote_cache_dir_is_scoped_per_daemon_and_path() {
    let root = std::path::Path::new("/cache");
    let addr1 = kernel::transport::SocketAddr::Wss("a.example.com".to_string());
    let addr2 = kernel::transport::SocketAddr::Wss("b.example.com".to_string());

    let dir = remote_cache_dir(root, &addr1, "/work/out.pdf");
    assert!(dir.starts_with(root.join("remote-attachments")));
    // Same path on another daemon, or another path on the same daemon,
    // must not collide.
    assert_ne!(dir, remote_cache_dir(root, &addr2, "/work/out.pdf"));
    assert_ne!(dir, remote_cache_dir(root, &addr1, "/work/other.pdf"));
    // Deterministic for the same inputs (cache survives app restarts).
    assert_eq!(dir, remote_cache_dir(root, &addr1, "/work/out.pdf"));
}

#[test]
fn cache_entry_name_encodes_content_identity() {
    let name = cache_entry_name(42, 1_700_000_000_000, "/work/reports/q3.pdf");
    assert_eq!(name, "1700000000000-42-q3.pdf");
    // Windows-style separators are also stripped.
    assert_eq!(cache_entry_name(1, 2, "C:\\work\\a.txt"), "2-1-a.txt");
    // A changed file gets a new name → stale copies never open.
    assert_ne!(
        cache_entry_name(42, 1, "/w/a.pdf"),
        cache_entry_name(43, 1, "/w/a.pdf")
    );
    assert_ne!(
        cache_entry_name(42, 1, "/w/a.pdf"),
        cache_entry_name(42, 2, "/w/a.pdf")
    );
    // Degenerate basename falls back to a placeholder.
    assert_eq!(cache_entry_name(1, 2, "/"), "2-1-file");
}
