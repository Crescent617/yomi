use crate::args::GlobalArgs;
use anyhow::Result;
use clap::Subcommand;
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
}

pub async fn run(cmd: DaemonCommands, global: &GlobalArgs) -> Result<()> {
    const IDLE_CHECK_INTERVAL: Duration = Duration::from_mins(1);
    const DAEMON_IDLE_TIMEOUT_SECS: u64 = 300;
    const SHUTDOWN_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
    const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

    match cmd {
        DaemonCommands::Start { auto_exit } => {
            // Guard against running multiple daemon instances
            let pid_file = crate::daemon::pid_file_path();
            if crate::daemon::try_connect().await.is_some() {
                tracing::info!("Daemon already running, refusing to start");
                println!("Daemon is already running");
                return Ok(());
            }
            if let Ok(s) = tokio::fs::read_to_string(&pid_file).await {
                if let Ok(pid) = s.trim().parse::<u32>() {
                    if crate::daemon::process_exists(pid) {
                        tracing::info!(pid = pid, "Daemon already running, refusing to start");
                        println!("Daemon is already running (PID {pid})");
                        return Ok(());
                    }
                }
                // Stale PID file — clean it up
                let _ = tokio::fs::remove_file(&pid_file).await;
                tracing::info!("Stale PID file, cleaning up");
            }

            if let Some(config_path) = &global.config {
                // Honor -c/--config; also persist it to the env so a
                // self-respawn after a restart request (which inherits env
                // but not CLI args) loads the same config file.
                std::env::set_var(kernel::config::env_names::CONFIG, config_path);
            }
            let (kernel, config, config_file) =
                kernel::init_kernel(global.config.as_ref(), true).await?;
            let config_file = config_file.or_else(|| Some(kernel::config::Config::write_path()));
            let _log_guard = kernel::utils::logging::init_logging(&config, "daemon", true)?;
            if let Some(path) = &config_file {
                tracing::info!("Loaded config from {}", path.display());
            }

            let addr = crate::daemon::socket_addr();

            // Bind listener FIRST so clients can connect while we initialize.
            let listener = kernel::transport::bind(&addr).await?;
            tracing::info!("Daemon listening on {addr}");

            // Write PID file so external tools can find and signal us.
            if let Some(parent) = pid_file.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&pid_file, std::process::id().to_string()).await?;

            let (restart_tx, mut restart_rx) = tokio::sync::mpsc::channel(1);
            let restart_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let server = kernel::server::KernelServer::with_lifecycle(
                Arc::clone(&kernel),
                config_file,
                Some(restart_tx),
            );
            server.start(&config).await;
            let shutdown = tokio_util::sync::CancellationToken::new();
            let restart_shutdown = shutdown.clone();
            let restart_requested_task = Arc::clone(&restart_requested);
            tokio::spawn(async move {
                if restart_rx.recv().await.is_some() {
                    restart_requested_task.store(true, std::sync::atomic::Ordering::Release);
                    restart_shutdown.cancel();
                }
            });

            let _signal_handle = kernel::utils::signal::spawn_signal_listener(shutdown.clone());

            if auto_exit {
                let server_for_exit = server.clone();
                let coord_for_exit = Arc::clone(&kernel);
                let shutdown_clone = shutdown.clone();
                tokio::spawn(async move {
                    tracing::info!(
                        "Daemon starting with {} active agent(s), idle check {}s",
                        coord_for_exit.live_session_count(),
                        DAEMON_IDLE_TIMEOUT_SECS
                    );
                    let mut interval = tokio::time::interval(IDLE_CHECK_INTERVAL);
                    interval.tick().await;
                    loop {
                        tokio::select! {
                            biased;
                            () = shutdown_clone.cancelled() => {
                                tracing::info!("Idle checker shutting down");
                                break;
                            }
                            _ = interval.tick() => {
                                let clients = server_for_exit.connection_count();
                                if clients == 0
                                    && coord_for_exit.live_session_count() == 0
                                {
                                    tracing::info!(
                                        "Auto-exiting daemon with no clients or sessions"
                                    );
                                    shutdown_clone.cancel();
                                    break;
                                }
                            }
                        }
                    }
                });
            }

            let serve_result = server.serve(listener, shutdown).await;
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

            tracing::info!(pid = std::process::id(), "Daemon shutting down gracefully");
            serve_result?;

            if restart_requested.load(std::sync::atomic::Ordering::Acquire) {
                tracing::info!("Restart requested through KernelApi; spawning replacement daemon");
                crate::daemon::spawn_daemon_with_auto_exit(auto_exit).await?;
            }
        }
        DaemonCommands::Stop => {
            tracing::info!("Stop command received");
            crate::daemon::graceful_shutdown().await?;
            println!("Daemon stopped");
        }
        DaemonCommands::Restart => {
            tracing::info!("Restart command received");
            crate::daemon::restart_daemon().await?;
            println!("Daemon restarted");
        }
        DaemonCommands::Status => {
            let status = crate::daemon::daemon_status().await?;
            println!("{status}");
        }
    }
    Ok(())
}
