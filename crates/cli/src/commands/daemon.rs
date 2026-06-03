use anyhow::Result;
use clap::Subcommand;
use kernel::client::CoordinatorApi;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Subcommand)]
pub enum DaemonCommands {
    /// Start daemon server (internal)
    Start {
        #[arg(long)]
        auto_exit: bool,
    },
    /// Stop daemon gracefully
    Stop,
    /// Restart daemon
    Restart,
    /// Check daemon status
    Status,
    /// Reload skills into running daemon
    Reload,
}

pub async fn run(cmd: DaemonCommands) -> Result<()> {
    match cmd {
        DaemonCommands::Start { auto_exit } => {
            const IDLE_CHECK_INTERVAL: Duration = Duration::from_mins(1);
            const DAEMON_IDLE_TIMEOUT_SECS: u64 = 300;
            const SHUTDOWN_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
            const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

            let working_dir = std::env::current_dir()?;
            let config_file = kernel::config::Config::discover_file();
            let config = crate::utils::load_config(config_file.as_ref(), &working_dir)?;
            tokio::fs::create_dir_all(&config.data_dir).await?;
            let _log_guard = crate::commands::tui::init_logging(&config)?;

            let addr = crate::daemon::socket_addr();

            // Bind listener FIRST so clients can connect while we initialize.
            let listener = kernel::transport::bind(&addr).await?;
            tracing::info!("Daemon listening on {addr}");

            let base_dir = config_file.as_ref().and_then(|p| p.parent()).map_or_else(
                || kernel::expand_tilde(kernel::DEFAULT_DATA_DIR),
                PathBuf::from,
            );
            let storage = kernel::StorageSet::open_with_config(&config.data_dir, &config).await?;
            let provider = crate::commands::tui::create_provider(&config)?;
            let task_store = Arc::new(kernel::TaskStore::new(&config.data_dir).await?);
            let skill_folders = crate::commands::tui::resolve_skill_folders(&config, &base_dir);

            // Load skills so the daemon starts with the same agent configuration
            // (including skills) that reload would produce.
            let agent_config = tokio::task::spawn_blocking({
                let config = config.clone();
                let base_dir = base_dir.clone();
                move || kernel::server::build_agent_config(&config, &base_dir)
            })
            .await?;

            let coordinator = Arc::new(kernel::Coordinator::new(
                &storage,
                provider,
                agent_config,
                Some(task_store),
                Some(config.agent.compactor.clone()),
                skill_folders,
                config.features.hooks.then(|| {
                    kernel::hooks::build_registry(
                        &config.hooks,
                        config.features.allow_command_hooks,
                    )
                }),
            ));

            let server =
                kernel::server::KernelServer::new(Arc::clone(&coordinator), config_file, base_dir);
            let shutdown = tokio_util::sync::CancellationToken::new();

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

            if auto_exit {
                let server_for_exit = server.clone();
                let coord_for_exit = Arc::clone(&coordinator);
                let shutdown_clone = shutdown.clone();
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(IDLE_CHECK_INTERVAL);
                    loop {
                        interval.tick().await;
                        let idle = coord_for_exit.idle_seconds();
                        let clients = server_for_exit.connection_count();
                        if idle >= DAEMON_IDLE_TIMEOUT_SECS
                            && clients == 0
                            && coord_for_exit.live_session_count() == 0
                        {
                            tracing::info!(
                                "Auto-exiting daemon after {idle}s idle with no clients or sessions"
                            );
                            shutdown_clone.cancel();
                            break;
                        }
                    }
                });
            }

            let serve_result = server.serve_listener(listener, shutdown).await;
            // Cancel all active connections so the process can actually exit.
            server.shutdown().await;
            let start = tokio::time::Instant::now();
            while server.connection_count() > 0 && start.elapsed() < SHUTDOWN_WAIT_TIMEOUT {
                tokio::time::sleep(SHUTDOWN_POLL_INTERVAL).await;
            }

            // Remove PID and socket files so external lifecycle tools
            // (graceful_shutdown, spawn_daemon, etc.) know we've exited.
            let _ = tokio::fs::remove_file(crate::daemon::pid_file_path()).await;
            if let kernel::transport::SocketAddr::Unix(path) = &addr {
                let _ = tokio::fs::remove_file(path).await;
            }

            tracing::info!("Daemon shutting down gracefully");
            serve_result?;
        }
        DaemonCommands::Stop => {
            crate::daemon::graceful_shutdown().await?;
            println!("Daemon stopped");
        }
        DaemonCommands::Restart => {
            crate::daemon::restart_daemon().await?;
            println!("Daemon restarted");
        }
        DaemonCommands::Status => {
            let status = crate::daemon::daemon_status().await?;
            println!("{status}");
        }
        DaemonCommands::Reload => {
            let coord =
                kernel::client::RemoteCoordinator::connect(&crate::daemon::socket_addr()).await?;
            coord.reload_agent_config().await?;
            println!("Skills reloaded in daemon");
        }
    }
    Ok(())
}
