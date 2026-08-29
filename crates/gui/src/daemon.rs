//! Daemon lifecycle management for yomi-gui.
//!
//! The GUI always connects to a daemon via IPC. Two strategies:
//! 1. Connect to an existing yomi daemon (external or CLI-started).
//! 2. Spawn a background daemon if none is running.
//!
//! There is no in-process fallback — the daemon is the only supported path.
//!
//! All cron operations go through the `KernelApi` so the same GUI code
//! works regardless of which strategy is used.

use anyhow::{Context, Result};
use kernel::transport::SocketAddr;
pub use kernel::transport::{pid_file_path, socket_addr};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(10);
const SPAWN_READY_INTERVAL: Duration = Duration::from_millis(100);
/// How long a restart waits for the old server task to exit before
/// spawning the new one anyway.
const RESTART_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Lifecycle slot for the daemon this GUI talks to.
enum DaemonSlot {
    /// Daemon spawned by this process and (presumably) running.
    Managed {
        shutdown: CancellationToken,
        /// Join handle of the background task running `serve()`. Awaiting it
        /// guarantees the old listener is gone and its pid/socket cleanup
        /// has run, so a follow-up spawn cannot race with it.
        serve_handle: tokio::task::JoinHandle<()>,
    },
    /// We owned the daemon but it is no longer running: a restart failed
    /// after the old server was stopped (e.g. the config file no longer
    /// parses). Kept so the user can fix the config and retry from the GUI
    /// instead of restarting the whole app.
    Dead,
}

/// Global handle for the in-process daemon server.
///
/// `None` when the GUI is connected to an external (e.g. CLI-started)
/// daemon; `Some` while this process owns the daemon lifecycle — even if
/// the server itself is currently dead.
static MANAGED_DAEMON: tokio::sync::Mutex<Option<DaemonSlot>> = tokio::sync::Mutex::const_new(None);
static DAEMON_LIFECYCLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

type RestartChannel = (
    tokio::sync::mpsc::Sender<()>,
    std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<()>>>,
);
static RESTART_CHANNEL: std::sync::OnceLock<RestartChannel> = std::sync::OnceLock::new();

fn restart_sender() -> tokio::sync::mpsc::Sender<()> {
    RESTART_CHANNEL
        .get_or_init(|| {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            (tx, std::sync::Mutex::new(Some(rx)))
        })
        .0
        .clone()
}

pub fn take_restart_receiver() -> Option<tokio::sync::mpsc::Receiver<()>> {
    RESTART_CHANNEL
        .get_or_init(|| {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            (tx, std::sync::Mutex::new(Some(rx)))
        })
        .1
        .lock()
        .ok()?
        .take()
}

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

/// Obtain a `KernelApi` for the GUI.
///
/// Tries two strategies in order:
/// 1. Connect to an existing daemon.
/// 2. Spawn a background daemon and connect to it.
///
/// Returns an error if neither succeeds. The GUI does not fall back to an
/// in-process kernel — the daemon is the only supported path.
pub async fn get_kernel() -> Result<(Arc<dyn kernel::client::KernelApi>, std::path::PathBuf), String>
{
    let addr = socket_addr();
    let default_config = kernel::config::Config::default();
    let data_dir = default_config.data_dir.clone();
    if try_connect().await.is_some() {
        tracing::info!("Connected to existing daemon at {addr}");
        return Ok((Arc::new(kernel::client::RemoteKernel::new(addr)), data_dir));
    }
    let config = spawn_daemon()
        .await
        .map_err(|e| format!("failed to spawn daemon: {e}"))?;
    tracing::info!("Connected to spawned daemon at {addr}");
    Ok((
        Arc::new(kernel::client::RemoteKernel::new(addr)),
        config.data_dir,
    ))
}

/// Start the kernel server in a background task.
/// If a daemon is already accepting connections, returns Ok immediately.
pub async fn spawn_daemon() -> Result<kernel::config::Config> {
    let _lifecycle_guard = DAEMON_LIFECYCLE.lock().await;
    spawn_daemon_inner().await
}

async fn spawn_daemon_inner() -> Result<kernel::config::Config> {
    // Install rustls crypto provider before any TLS operations.
    // Required by rustls 0.23+ when multiple crypto providers are available.
    let _ = rustls::crypto::ring::default_provider().install_default();

    if try_connect().await.is_some() {
        tracing::info!("daemon already running, skipping spawn");
        return Ok(kernel::config::Config::default());
    }

    let (kernel, config, config_file) = kernel::init_kernel(None, true).await?;
    let config_file = config_file.or_else(|| Some(kernel::config::Config::write_path()));

    let addr = socket_addr();
    // Socket auth only applies to ws/wss listeners; unix sockets rely on
    // filesystem permissions, so skip it entirely there.
    let auth = if matches!(
        addr,
        kernel::transport::SocketAddr::Ws(_) | kernel::transport::SocketAddr::Wss(_)
    ) {
        match config.socket_auth_hash.as_deref() {
            Some(hash) => {
                if kernel::transport::is_valid_hash_format(hash) {
                    Some(kernel::transport::auth_verifier(hash))
                } else {
                    // In-process daemon: we can't exit the app over a bad
                    // config value, and a malformed hash would fail closed —
                    // rejecting every client with no diagnosable signal. Log
                    // loudly and run without socket auth (same as not
                    // configuring it).
                    tracing::error!(
                        "invalid socket_auth_hash format (expected `blake3:<64 hex chars>`); \
                         starting daemon WITHOUT socket auth"
                    );
                    None
                }
            }
            None => {
                tracing::warn!("{}", kernel::transport::NO_SOCKET_AUTH_WARNING);
                None
            }
        }
    } else {
        None
    };
    let listener = kernel::transport::bind(&addr, auth)
        .await
        .with_context(|| format!("Failed to bind daemon listener on {addr}"))?;
    tracing::info!("Daemon listening on {addr}");

    let server = kernel::server::KernelServer::with_lifecycle(
        Arc::clone(&kernel),
        config_file,
        Some(restart_sender()),
    );
    server.start(&config).await;
    let shutdown = CancellationToken::new();

    // Write PID file
    let pid = std::process::id();
    tokio::fs::write(pid_file_path(), pid.to_string()).await?;

    // Run server in background
    let server_clone = server.clone();
    let addr_clone = addr.clone();
    let serve_shutdown = shutdown.clone();
    let serve_handle = tokio::spawn(async move {
        let result = server_clone.serve(vec![listener], serve_shutdown).await;
        if let Err(e) = result {
            tracing::error!("Daemon server error: {e}");
        }
        let _ = tokio::fs::remove_file(pid_file_path()).await;
        if let SocketAddr::Unix(ref path) = addr_clone {
            let _ = tokio::fs::remove_file(path).await;
        }
        tracing::info!("Daemon server stopped");
    });

    {
        let mut guard = MANAGED_DAEMON.lock().await;
        *guard = Some(DaemonSlot::Managed {
            shutdown,
            serve_handle,
        });
    }

    // Wait for server to be ready
    let start = tokio::time::Instant::now();
    while start.elapsed() < SPAWN_READY_TIMEOUT {
        if try_connect().await.is_some() {
            tracing::info!("daemon ready after {:?}", start.elapsed());
            return Ok(config);
        }
        sleep(SPAWN_READY_INTERVAL).await;
    }

    tracing::warn!("daemon started but did not become ready within {SPAWN_READY_TIMEOUT:?}");
    Err(anyhow::anyhow!(
        "daemon started but did not become ready within {SPAWN_READY_TIMEOUT:?}"
    ))
}

/// Whether this process owns the daemon lifecycle (i.e. the daemon was
/// spawned here rather than started externally).
///
/// Stays `true` even when a failed restart left the server dead, so the
/// user can fix the config and retry from the GUI.
pub async fn is_managed() -> bool {
    MANAGED_DAEMON.lock().await.is_some()
}

/// Remove pid/socket files, but only when the pid file points to this
/// process, so we never clean up files written by another (e.g.
/// CLI-started) daemon.
async fn cleanup_own_daemon_files() {
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
}

/// Stop the daemon that was spawned by this process.
///
/// If the GUI connected to an external daemon, this is a no-op.
pub async fn stop_daemon() -> Result<()> {
    let _lifecycle_guard = DAEMON_LIFECYCLE.lock().await;
    stop_daemon_inner().await
}

async fn stop_daemon_inner() -> Result<()> {
    let slot = {
        let mut guard = MANAGED_DAEMON.lock().await;
        guard.take()
    };

    let Some(slot) = slot else {
        // Not our daemon — nothing to clean up.
        return Ok(());
    };

    if let DaemonSlot::Managed {
        shutdown,
        mut serve_handle,
    } = slot
    {
        shutdown.cancel();
        if tokio::time::timeout(RESTART_STOP_TIMEOUT, &mut serve_handle)
            .await
            .is_err()
        {
            serve_handle.abort();
            let _ = serve_handle.await;
        }
    }
    // The serve task removes the pid/socket files itself on exit; clean up
    // defensively in case it does not get to run (the process is exiting).
    cleanup_own_daemon_files().await;

    Ok(())
}

/// Restart the daemon spawned by this process, reloading the config from
/// disk.
///
/// Fails when the GUI is connected to an external daemon. All running
/// sessions and tasks are interrupted; persisted data is not affected.
/// Existing `RemoteKernel` clients reconnect lazily on their next call, so
/// no client-side state needs to be rebuilt.
///
/// If the new daemon fails to come up (e.g. the config no longer parses),
/// ownership of the lifecycle is retained so a later call can retry after
/// the config is fixed.
pub async fn restart_daemon() -> Result<kernel::config::Config> {
    let _lifecycle_guard = DAEMON_LIFECYCLE.lock().await;
    restart_daemon_inner().await
}

async fn restart_daemon_inner() -> Result<kernel::config::Config> {
    let slot = {
        let mut guard = MANAGED_DAEMON.lock().await;
        guard.take()
    };

    let Some(slot) = slot else {
        anyhow::bail!("daemon was started externally and cannot be restarted from the GUI");
    };

    if let DaemonSlot::Managed {
        shutdown,
        mut serve_handle,
    } = slot
    {
        // Stop the old server and wait for its task to fully exit, so the
        // new spawn never races with the old listener or its pid/socket
        // cleanup.
        shutdown.cancel();
        if tokio::time::timeout(RESTART_STOP_TIMEOUT, &mut serve_handle)
            .await
            .is_err()
        {
            tracing::warn!("timed out waiting for old daemon server to exit; aborting it and spawning new one anyway");
            // Abort the stuck task: letting it run would execute its
            // deferred pid/socket cleanup *after* the new daemon bound the
            // same paths, deleting the new daemon's socket file. We clean
            // up ourselves instead.
            serve_handle.abort();
            let _ = serve_handle.await;
            cleanup_own_daemon_files().await;
        }
    }

    kernel::config::Config::clear_injected_env();
    let result = spawn_daemon_inner().await;

    if result.is_err() {
        // Keep ownership unless the failed spawn actually registered a new
        // server (ready-timeout case), so the user can fix the config and
        // retry instead of restarting the app.
        let mut guard = MANAGED_DAEMON.lock().await;
        if guard.is_none() {
            *guard = Some(DaemonSlot::Dead);
        }
    }

    result
}
