use tokio_util::sync::CancellationToken;

/// Spawn a background task that listens for OS signals (SIGTERM/SIGINT on Unix,
/// Ctrl-C on Windows) and cancels the provided `shutdown` token when any arrives.
/// The task also exits if `shutdown` is already cancelled from the outside.
///
/// Returns the `JoinHandle` of the spawned task so the caller can abort it if needed.
pub fn spawn_signal_listener(shutdown: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut sigterm =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to register SIGTERM handler: {e}");
                        return;
                    }
                };
            let mut sigint =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to register SIGINT handler: {e}");
                        return;
                    }
                };
            let shutdown_clone = shutdown.clone();
            tokio::select! {
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM, initiating graceful shutdown");
                }
                _ = sigint.recv() => {
                    tracing::info!("Received SIGINT, initiating graceful shutdown");
                }
                () = shutdown_clone.cancelled() => {
                    // shutdown triggered externally
                }
            }
        }
        #[cfg(not(unix))]
        {
            let shutdown_clone = shutdown.clone();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Received Ctrl-C, initiating graceful shutdown");
                }
                () = shutdown_clone.cancelled() => {
                    // shutdown triggered externally
                }
            }
        }
        shutdown.cancel();
    })
}
