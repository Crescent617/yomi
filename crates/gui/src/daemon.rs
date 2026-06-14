//! Daemon lifecycle management for yomi-gui.
//!
//! Supports three connection strategies:
//! 1. Connect to an existing yomi daemon (external or CLI-started).
//! 2. Spawn a background daemon if none is running.
//! 3. Fall back to an in-process coordinator for single-tenant builds.
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

/// Initialised core components for the GUI (shared by in-process and IPC paths).
///
/// DESIGN PRINCIPLE: The GUI never holds a `CronStore` directly. All cron
/// operations go through the `CoordinatorApi`, so the same code works for both
/// local (in-process) and remote (IPC) kernel connections.
pub struct KernelInit {
    pub coordinator: Arc<kernel::Coordinator>,
    /// Shutdown token for the cron subsystem (only present when cron was started).
    #[allow(dead_code)]
    pub cron_shutdown: Option<CancellationToken>,
    /// Handle to the cron scheduler so callers can trigger reloads after mutations.
    #[allow(dead_code)]
    pub cron_scheduler: Option<Arc<kernel::cron::CronScheduler>>,
}

/// Obtain a `CoordinatorApi` for the GUI.
///
/// Tries three strategies in order:
/// 1. Connect to an existing daemon.
/// 2. Spawn a background daemon and connect to it.
/// 3. Fall back to an in-process coordinator.
///
/// Returns an error only if all three strategies fail.
pub async fn get_coordinator() -> Result<Arc<dyn kernel::client::CoordinatorApi>, String> {
    let addr = socket_addr();
    if try_connect().await.is_some() {
        return Ok(Arc::new(kernel::client::RemoteCoordinator::new(addr)));
    }
    if spawn_daemon().await.is_ok() {
        return Ok(Arc::new(kernel::client::RemoteCoordinator::new(addr)));
    }
    let init = init_coordinator(true)
        .await
        .map_err(|e| format!("failed to initialise kernel coordinator: {e}"))?;
    Ok(init.coordinator)
}

/// Initialise a `Coordinator` in-process without any IPC.
///
/// Opens storage, loads config, and builds the agent. The `cron_store`
/// is always created in the coordinator; the caller decides whether to
/// start the cron scheduler + worker (`enable_cron` for in-process mode).
/// For daemon mode, `KernelServer` checks `cron_store` itself and starts
/// cron if available.
///
/// This is the zero-overhead path for a single-tenant GUI. To support
/// remote connections or multiple clients, use `spawn_daemon()` instead.
pub async fn init_coordinator(enable_cron: bool) -> Result<KernelInit> {
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

    let coordinator = kernel::Coordinator::new(
        &storage,
        provider,
        agent_config,
        Some(task_store),
        Some(config.agent.compactor.clone()),
        skill_folders,
        config.features.hooks.then(|| {
            kernel::hooks::build_registry(&config.hooks, config.features.allow_command_hooks)
        }),
    );

    // Start cron subsystem only when requested (GUI in-process mode).
    let (cron_shutdown, cron_scheduler) = if enable_cron {
        coordinator
            .cron_store()
            .map(|store| {
                let (task_tx, task_rx) = tokio::sync::mpsc::channel(64);
                let shutdown = CancellationToken::new();
                let scheduler = Arc::new(kernel::cron::CronScheduler::new(
                    store.clone(),
                    task_tx,
                    shutdown.clone(),
                ));

                let sched_clone = Arc::clone(&scheduler);
                tokio::spawn(async move { sched_clone.run().await });

                let worker = kernel::cron::CronWorker::new(
                    Arc::clone(&coordinator) as Arc<dyn kernel::cron::CronExecutor>,
                    task_rx,
                    store.clone(),
                    Some(Arc::clone(&scheduler)),
                    shutdown.clone(),
                );
                tokio::spawn(async move { worker.run().await });

                (shutdown, scheduler)
            })
            .map_or((None, None), |(s, sched)| (Some(s), Some(sched)))
    } else {
        (None, None)
    };

    Ok(KernelInit {
        coordinator,
        cron_shutdown,
        cron_scheduler,
    })
}

/// Start the kernel server in a background task.
/// If a daemon is already accepting connections, returns Ok immediately.
pub async fn spawn_daemon() -> Result<()> {
    if try_connect().await.is_some() {
        tracing::info!("daemon already running, skipping spawn");
        return Ok(());
    }

    let KernelInit { coordinator, .. } = init_coordinator(false).await?;
    let config_file = kernel::config::Config::discover_file();
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
                let mut sigterm = tokio::signal::unix::signal(
                    tokio::signal::unix::SignalKind::terminate(),
                )
                .expect("Failed to register SIGTERM handler");
                let mut sigint = tokio::signal::unix::signal(
                    tokio::signal::unix::SignalKind::interrupt(),
                )
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
        tracing::warn!(
            "No API key configured — using NoKeyProvider (sessions will fail to send messages)"
        );
        return Ok(Arc::new(kernel::NoKeyProvider));
    }

    let provider: Arc<dyn kernel::Provider> = match config.agent.model.provider {
        kernel::ModelProvider::OpenAI => Arc::new(kernel::OpenAIProvider::new()?),
        kernel::ModelProvider::Anthropic => Arc::new(kernel::AnthropicProvider::new()?),
    };

    Ok(provider)
}
