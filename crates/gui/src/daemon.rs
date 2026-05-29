//! Daemon lifecycle management for yomi-gui.
//!
//! Directly starts the kernel server inside the GUI process so the GUI and CLI
//! share a single kernel. Sessions and state survive GUI restarts.

use anyhow::{Context, Result};
use kernel::transport::SocketAddr;
pub use kernel::transport::{socket_addr, pid_file_path};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(10);
const SPAWN_READY_INTERVAL: Duration = Duration::from_millis(100);

/// Global shutdown token for the in-process daemon server.
static DAEMON_SHUTDOWN: std::sync::Mutex<Option<CancellationToken>> = std::sync::Mutex::new(None);

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

/// Start the kernel server directly in a background tokio task.
/// If a daemon is already accepting connections, returns Ok immediately.
pub async fn spawn_daemon() -> Result<()> {
    if try_connect().await.is_some() {
        tracing::info!("daemon already running, skipping spawn");
        return Ok(());
    }

    let working_dir = std::env::current_dir()?;
    let config_file = kernel::config::Config::discover_file();
    let mut config = if let Some(ref path) = config_file {
        kernel::config::Config::from_file(path)
            .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?
    } else {
        kernel::config::Config::default()
    };
    config.apply_env_overrides();
    config.finalize(&working_dir);

    tokio::fs::create_dir_all(&config.data_dir).await?;

    let addr = socket_addr();
    let listener = kernel::transport::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind daemon listener on {addr}"))?;
    tracing::info!("Daemon listening on {addr}");

    let base_dir = config_file.as_ref().and_then(|p| p.parent()).map_or_else(
        || kernel::expand_tilde(kernel::DEFAULT_DATA_DIR),
        PathBuf::from,
    );

    let storage = kernel::StorageSet::open_with_config(&config.data_dir, &config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open storage: {e}"))?;
    let provider = create_provider(&config)?;
    let task_store = Arc::new(
        kernel::TaskStore::new(&config.data_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create task store: {e}"))?,
    );
    let skill_folders = resolve_skill_folders(&config, &base_dir);

    let agent_config = tokio::task::spawn_blocking({
        let config = config.clone();
        let base_dir = base_dir.clone();
        move || kernel::server::build_agent_config(&config, &base_dir)
    })
    .await
    .context("Failed to build agent config in blocking task")?;

    let coordinator = Arc::new(kernel::Coordinator::new(
        &storage,
        provider,
        agent_config,
        Some(task_store),
        Some(config.agent.compactor.clone()),
        skill_folders,
        config
            .features
            .hooks
            .then(|| kernel::hooks::build_registry(&config.hooks)),
    ));

    let server = kernel::server::KernelServer::new(Arc::clone(&coordinator), config_file, base_dir);
    let shutdown = CancellationToken::new();

    {
        let mut guard = DAEMON_SHUTDOWN.lock().unwrap();
        *guard = Some(shutdown.clone());
    }

    // Signal handler
    {
        let shutdown_sig = shutdown.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm = signal(SignalKind::terminate())
                    .expect("Failed to register SIGTERM handler");
                let mut sigint = signal(SignalKind::interrupt())
                    .expect("Failed to register SIGINT handler");
                tokio::select! {
                    _ = sigterm.recv() => {
                        tracing::info!("Received SIGTERM, initiating graceful shutdown");
                    }
                    _ = sigint.recv() => {
                        tracing::info!("Received SIGINT, initiating graceful shutdown");
                    }
                }
            }
            #[cfg(windows)]
            {
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("Received Ctrl-C, initiating graceful shutdown");
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

/// Force-stop the daemon.
pub async fn stop_daemon() -> Result<()> {
    let pid_file = pid_file_path();
    {
        let guard = DAEMON_SHUTDOWN.lock().unwrap();
        if let Some(token) = guard.clone() {
            token.cancel();
        }
    }
    let _ = tokio::fs::remove_file(&pid_file).await;
    if let SocketAddr::Unix(ref path) = socket_addr() {
        let _ = tokio::fs::remove_file(path).await;
    }

    let start = tokio::time::Instant::now();
    while try_connect().await.is_some() && start.elapsed() < Duration::from_secs(2) {
        sleep(Duration::from_millis(50)).await;
    }

    Ok(())
}

/// Gracefully shut down the daemon.
pub async fn graceful_shutdown() -> Result<()> {
    let pid_file = pid_file_path();
    if !pid_file.exists() {
        tracing::info!("no daemon found, nothing to stop");
        return Ok(());
    }

    {
        let guard = DAEMON_SHUTDOWN.lock().unwrap();
        if let Some(token) = guard.clone() {
            tracing::info!("cancelling in-process daemon...");
            token.cancel();
        } else {
            tracing::info!("no daemon shutdown token found, cleaning up stale files");
        }
    }

    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while pid_file.exists() {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    if pid_file.exists() {
        tracing::warn!("daemon did not exit gracefully, falling back to stop");
        stop_daemon().await?;
    } else {
        tracing::info!("daemon shut down gracefully");
    }

    Ok(())
}

/// Check daemon status.
pub async fn daemon_status() -> Result<String> {
    let addr = socket_addr();
    let pid_file = pid_file_path();

    if kernel::transport::connect(&addr).await.is_ok() {
        return Ok("daemon is running".to_string());
    }

    let stale = if pid_file.exists() {
        match tokio::fs::read_to_string(&pid_file).await {
            Ok(s) => {
                match s.trim().parse::<u32>() {
                    Ok(pid) => {
                        if !process_exists(pid) {
                            true
                        } else if pid == std::process::id() {
                            // In-process server is not responding
                            true
                        } else {
                            false
                        }
                    }
                    Err(_) => true,
                }
            }
            Err(_) => true,
        }
    } else {
        false
    };

    if stale {
        let _ = tokio::fs::remove_file(&pid_file).await;
        if let SocketAddr::Unix(ref path) = addr {
            let _ = tokio::fs::remove_file(path).await;
        }
        Ok("daemon is not running (stale PID cleaned)".to_string())
    } else if pid_file.exists() {
        Ok("daemon may be starting up".to_string())
    } else {
        Ok("daemon is not running".to_string())
    }
}

fn resolve_skill_folders(
    config: &kernel::config::Config,
    base_dir: &std::path::Path,
) -> Vec<PathBuf> {
    config
        .skill_folders()
        .iter()
        .map(PathBuf::from)
        .map(|p| if p.is_relative() { base_dir.join(p) } else { p })
        .collect()
}

fn create_provider(config: &kernel::config::Config) -> Result<Arc<dyn kernel::Provider>> {
    if !config.has_api_key() {
        anyhow::bail!("API key not configured. Please set it via config or environment variable.");
    }

    let provider: Arc<dyn kernel::Provider> = match config.agent.model.provider {
        kernel::ModelProvider::OpenAI => Arc::new(kernel::OpenAIProvider::new()?),
        kernel::ModelProvider::Anthropic => Arc::new(kernel::AnthropicProvider::new()?),
    };

    Ok(provider)
}
