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
    /// Reload skills into running daemon
    Reload,
}

pub async fn run(cmd: DaemonCommands) -> Result<()> {
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

            let (coordinator, config, _config_file) =
                kernel::init_coordinator(None, true, true).await?;
            let _log_guard = kernel::logging::init_logging(&config, "daemon", true)?;

            let addr = crate::daemon::socket_addr();

            // Bind listener FIRST so clients can connect while we initialize.
            let listener = kernel::transport::bind(&addr).await?;
            tracing::info!("Daemon listening on {addr}");

            // Write PID file so external tools can find and signal us.
            if let Some(parent) = pid_file.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&pid_file, std::process::id().to_string()).await?;

            let server = kernel::server::KernelServer::new(Arc::clone(&coordinator));
            let shutdown = tokio_util::sync::CancellationToken::new();

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
                                // shutdown triggered by idle auto-exit or external
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

            if auto_exit {
                let server_for_exit = server.clone();
                let coord_for_exit = Arc::clone(&coordinator);
                let shutdown_clone = shutdown.clone();
                tokio::spawn(async move {
                    tracing::info!(
                        "Daemon starting with {} session(s), idle check {}s",
                        coord_for_exit.idle_seconds(),
                        DAEMON_IDLE_TIMEOUT_SECS
                    );
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
        DaemonCommands::Reload => {
            println!("Restart the daemon to apply configuration changes.");
            println!("Use: yomi daemon restart");
        }
    }
    Ok(())
}
