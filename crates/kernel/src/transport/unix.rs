#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tokio::net::UnixListener;

/// Bind a Unix socket at the given path, removing stale sockets first.
/// Sets permissions to 0o600 (owner-only) before converting to the async
/// tokio wrapper.
pub async fn bind_socket(path: &Path) -> std::io::Result<UnixListener> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Remove stale socket if it exists.
    if let Err(e) = tokio::fs::remove_file(path).await {
        tracing::debug!("Failed to remove stale socket: {e}");
    }

    // Use the std (synchronous) UnixListener so we can chmod immediately
    // before converting to the async tokio wrapper.
    let std_listener = std::os::unix::net::UnixListener::bind(path)?;
    let perms = std::fs::Permissions::from_mode(0o600);
    tokio::fs::set_permissions(path, perms).await?;
    std_listener.set_nonblocking(true)?;
    UnixListener::from_std(std_listener)
}
