//! Daemon lifecycle management for yomi-gui.
//!
//! The GUI always connects to a daemon via IPC. Two strategies:
//! 1. Connect to an existing yomi daemon (external or CLI-started).
//! 2. Spawn a background daemon if none is running.
//!
//! There is no in-process fallback — the daemon is the only supported path.
//!
//! All cron operations go through the `CoordinatorApi` so the same GUI code
//! works regardless of which strategy is used.

use anyhow::{Context, Result};
use kernel::transport::SocketAddr;
pub use kernel::transport::{pid_file_path, socket_addr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(10);
const SPAWN_READY_INTERVAL: Duration = Duration::from_millis(100);

/// Global shutdown token for the in-process daemon server.
static DAEMON_SHUTDOWN: tokio::sync::Mutex<Option<CancellationToken>> =
    tokio::sync::Mutex::const_new(None);

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
}

#[cfg(not(unix))]
fn process_exists(_pid: u32) -> bool {
    false
}

/// Try connecting to the daemon.
pub async fn try_connect() -> Option<kernel::transport::Stream> {
    let addr = socket_addr();
    match kernel::transport::connect(&addr).await {
        Ok(stream) => Some(stream),
        Err(_) => {
            let pid_file = pid_file_path();
            if pid_file.exists() {
                if let Ok(s) = tokio::fs::read_to_string(&pid_file).await {
                    match s.trim().parse::<u32>() {
                        Ok(pid) if !process_exists(pid) => {
                            let _ = tokio::fs::remove_file(&pid_file).await;
                        }
                        Ok(_) => {}
                        Err(_) => {
                            let _ = tokio::fs::remove_file(&pid_file).await;
                        }
                    }
                }
            }
            None
        }
    }
}

/// Obtain a `CoordinatorApi` for the GUI.
///
/// Tries two strategies in order:
/// 1. Connect to an existing daemon.
/// 2. Spawn a background daemon and connect to it.
///
/// Returns an error if neither succeeds. The GUI does not fall back to an
/// in-process coordinator — the daemon is the only supported path.
pub async fn get_coordinator() -> Result<Arc<dyn kernel::client::CoordinatorApi>, String> {
    let addr = socket_addr();
    if try_connect().await.is_some() {
        tracing::info!("Connected to existing daemon at {addr}");
        return Ok(Arc::new(kernel::client::RemoteCoordinator::new(addr)));
    }
    spawn_daemon()
        .await
        .map_err(|e| format!("failed to spawn daemon: {e}"))?;
    tracing::info!("Connected to spawned daemon at {addr}");
    Ok(Arc::new(kernel::client::RemoteCoordinator::new(addr)))
}

/// Start the kernel server in a background task.
/// If a daemon is already accepting connections, returns Ok immediately.
pub async fn spawn_daemon() -> Result<()> {
    if try_connect().await.is_some() {
        tracing::info!("daemon already running, skipping spawn");
        return Ok(());
    }

    let (coordinator, _config, config_file) = kernel::init_coordinator(None, true).await?;
    let base_dir = config_file.as_ref().and_then(|p| p.parent()).map_or_else(
        || kernel::expand_tilde(kernel::DEFAULT_DATA_DIR),
        PathBuf::from,
    );

    let addr = socket_addr();
    let listener = kernel::transport::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind daemon listener on {addr}"))?;
    tracing::info!("Daemon listening on {addr}");

    let server = kernel::server::KernelServer::new(Arc::clone(&coordinator), config_file, base_dir);
    let shutdown = CancellationToken::new();

    {
        let mut guard = DAEMON_SHUTDOWN.lock().await;
        *guard = Some(shutdown.clone());
    }

    // Signal handler
    {
        let shutdown_sig = shutdown.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                let mut sigterm =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("Failed to register SIGTERM handler");
                let mut sigint =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                        .expect("Failed to register SIGINT handler");
                let shutdown_clone = shutdown_sig.clone();
                tokio::select! {
                    _ = sigterm.recv() => {
                        tracing::info!("Received SIGTERM, initiating graceful shutdown");
                    }
                    _ = sigint.recv() => {
                        tracing::info!("Received SIGINT, initiating graceful shutdown");
                    }
                    () = shutdown_clone.cancelled() => {
                        // shutdown triggered by stop_daemon or auto_exit
                    }
                }
            }
            #[cfg(not(unix))]
            {
                let shutdown_clone = shutdown_sig.clone();
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        tracing::info!("Received Ctrl-C, initiating graceful shutdown");
                    }
                    () = shutdown_clone.cancelled() => {
                        // shutdown triggered externally
                    }
                }
            }
            shutdown_sig.cancel();
        });
    }

    // Write PID file
    let pid = std::process::id();
    tokio::fs::write(pid_file_path(), pid.to_string()).await?;

    // Run server in background
    let server_clone = server.clone();
    let addr_clone = addr.clone();
    tokio::spawn(async move {
        let result = server_clone.serve_listener(listener, shutdown).await;
        if let Err(e) = result {
            tracing::error!("Daemon server error: {e}");
        }
        server_clone.shutdown().await;
        let _ = tokio::fs::remove_file(pid_file_path()).await;
        if let SocketAddr::Unix(ref path) = addr_clone {
            let _ = tokio::fs::remove_file(path).await;
        }
        tracing::info!("Daemon server stopped");
    });

    // Wait for server to be ready
    let start = tokio::time::Instant::now();
    while start.elapsed() < SPAWN_READY_TIMEOUT {
        if try_connect().await.is_some() {
            tracing::info!("daemon ready after {:?}", start.elapsed());
            return Ok(());
        }
        sleep(SPAWN_READY_INTERVAL).await;
    }

    tracing::warn!("daemon started but did not become ready within {SPAWN_READY_TIMEOUT:?}");
    Err(anyhow::anyhow!(
        "daemon started but did not become ready within {SPAWN_READY_TIMEOUT:?}"
    ))
}

/// Stop the daemon that was spawned by this process.
///
/// Only deletes the PID / socket files when they point to the current process,
/// so we never clean up a file written by another (e.g. CLI-started) daemon.
/// If the GUI connected to an external daemon, this is a no-op.
pub async fn stop_daemon() -> Result<()> {
    let guard = DAEMON_SHUTDOWN.lock().await;
    let token = guard.clone();
    drop(guard);

    let Some(token) = token else {
        // Not our daemon — nothing to clean up.
        return Ok(());
    };

    token.cancel();

    let pid_file = pid_file_path();
    let own_pid = std::process::id().to_string();
    let should_remove = match tokio::fs::read_to_string(&pid_file).await {
        Ok(content) => content.trim() == own_pid,
        Err(_) => false,
    };

    if should_remove {
        let _ = tokio::fs::remove_file(&pid_file).await;
        if let SocketAddr::Unix(ref path) = socket_addr() {
            let _ = tokio::fs::remove_file(path).await;
        }
    }

    // Clear the global token so subsequent calls know no daemon is managed
    let mut guard = DAEMON_SHUTDOWN.lock().await;
    *guard = None;

    Ok(())
}
