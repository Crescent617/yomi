//! Daemon lifecycle management for yomi.

use anyhow::{Context, Result};
pub use kernel::transport::{pid_file_path, socket_addr};
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// How long to wait for graceful shutdown before falling back to kill.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
/// Polling interval while waiting for graceful shutdown.
const GRACEFUL_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Check whether a process with the given PID exists.
#[cfg(unix)]
pub fn process_exists(pid: u32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
}

#[cfg(not(unix))]
pub fn process_exists(_pid: u32) -> bool {
    // We cannot reliably detect process liveness on Windows without
    // adding heavy dependencies (OpenProcess / GetExitCodeProcess).
    // Callers should use `try_connect()` as the ground-truth signal.
    false
}

/// Clean up the PID file if it is stale (non-existent process).
/// Returns `true` if the file was removed, `false` if it is still valid or missing.
pub async fn cleanup_stale_pid_file() -> bool {
    let pid_file = pid_file_path();
    if !pid_file.exists() {
        return false;
    }
    let should_remove = match tokio::fs::read_to_string(&pid_file).await {
        Ok(s) => match s.trim().parse::<u32>() {
            Ok(pid) => !process_exists(pid),
            Err(_) => true,
        },
        Err(_) => true,
    };
    if should_remove {
        let _ = tokio::fs::remove_file(&pid_file).await;
        tracing::info!("Removed stale PID file");
    }
    should_remove
}

/// Try connecting to the daemon.
pub async fn try_connect() -> Option<kernel::transport::Stream> {
    let addr = socket_addr();
    match kernel::transport::connect(&addr).await {
        Ok(stream) => Some(stream),
        Err(_) => {
            let _ = cleanup_stale_pid_file().await;
            None
        }
    }
}

/// Spawn the daemon as a fully detached background process.
/// If a daemon is already accepting connections, returns Ok immediately.
/// Otherwise spawns a new process and polls until the socket is ready
/// (up to 10 s) so callers never race with daemon initialisation.
pub async fn spawn_daemon() -> Result<()> {
    spawn_daemon_with_auto_exit(true).await
}

pub async fn spawn_daemon_with_auto_exit(auto_exit: bool) -> Result<()> {
    const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(10);
    const SPAWN_READY_INTERVAL: Duration = Duration::from_millis(100);

    if try_connect().await.is_some() {
        tracing::info!("Daemon already running, skipping spawn");
        return Ok(());
    }

    let mut current_exe = std::env::current_exe().context("Failed to get current executable")?;

    // On Linux `current_exe` may return a `/proc/self/exe` symlink that has
    // the `(deleted)` suffix when the binary has been replaced since launch
    // (e.g. after a fresh cargo install).  In that case `spawn` fails with
    // ENOENT.  Fall back to argv[0] when the resolved path is missing.
    if !current_exe.exists() {
        if let Some(argv0) = std::env::args_os().next() {
            tracing::warn!(
                "current_exe {} does not exist, falling back to argv[0] {:?}",
                current_exe.display(),
                argv0
            );
            current_exe = PathBuf::from(argv0);
        }
    }

    let mut cmd = std::process::Command::new(&current_exe);
    cmd.arg("daemon").arg("start");
    if auto_exit {
        cmd.arg("--auto-exit");
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid().map_err(std::io::Error::other)?;
                Ok(())
            });
        }
    }

    for name in kernel::config::Config::injected_env_names() {
        cmd.env_remove(name);
    }
    let mut child = cmd.spawn().context("Failed to spawn daemon process")?;
    let pid = child.id();
    tracing::info!("Spawned daemon process (PID {pid})");

    // Poll until the daemon socket is actually accepting connections.
    // Daemon initialisation (storage, provider, skills) can take a few
    // seconds, so we allow up to 10 s.
    let start = tokio::time::Instant::now();
    while start.elapsed() < SPAWN_READY_TIMEOUT {
        if try_connect().await.is_some() {
            tracing::info!("Daemon ready after {:?}", start.elapsed());
            return Ok(());
        }
        sleep(SPAWN_READY_INTERVAL).await;
    }

    // Daemon failed to become ready — clean up the orphan process.
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || {
            let _ = child.kill();
            let _ = child.wait();
        }),
    )
    .await;
    tracing::warn!(
        "Daemon spawned (PID {pid}) but did not become ready within {SPAWN_READY_TIMEOUT:?}"
    );
    Err(anyhow::anyhow!(
        "Daemon spawned (PID {pid}) but did not become ready within {SPAWN_READY_TIMEOUT:?}"
    ))
}

/// Force-stop the daemon and wait for the process to actually exit.
pub async fn stop_daemon() -> Result<()> {
    let pid_file = pid_file_path();
    let pid = match tokio::fs::read_to_string(&pid_file).await {
        Ok(s) => s.trim().parse::<u32>().ok(),
        Err(_) => None,
    };

    #[cfg(unix)]
    if let Some(pid) = pid {
        tracing::info!("Sending SIGKILL to daemon (PID {pid})...");
        let signal_result = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        if let Err(error) = signal_result {
            anyhow::bail!("failed to kill daemon process {pid}: {error}");
        }
    }

    #[cfg(windows)]
    if let Some(pid) = pid {
        tracing::info!("Sending kill signal to daemon (PID {pid})...");
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
    }

    // Wait for the process to actually exit so a subsequent spawn
    // doesn't race with the old process holding the socket.
    if let Some(pid) = pid {
        let start = tokio::time::Instant::now();
        while process_exists(pid) && start.elapsed() < Duration::from_secs(2) {
            sleep(Duration::from_millis(50)).await;
        }
        if process_exists(pid) {
            anyhow::bail!("daemon process {pid} is still running after SIGKILL");
        }
    }

    // Only remove PID file after confirming the process is gone.
    let _ = tokio::fs::remove_file(&pid_file).await;

    tracing::info!("Daemon force-stopped");
    Ok(())
}

/// Gracefully shut down the daemon.
/// Falls back to `stop_daemon` if the daemon does not exit.
pub async fn graceful_shutdown() -> Result<()> {
    let pid_file = pid_file_path();
    if !pid_file.exists() {
        tracing::info!("No daemon found, nothing to stop");
        return Ok(());
    }

    let pid = match tokio::fs::read_to_string(&pid_file).await {
        Ok(s) => s.trim().parse::<u32>().ok(),
        Err(_) => None,
    };

    #[cfg(unix)]
    if let Some(pid) = pid {
        tracing::info!("Sending SIGTERM to daemon (PID {pid})...");
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );
    }

    #[cfg(windows)]
    if let Some(pid) = pid {
        tracing::info!("Sending graceful shutdown to daemon (PID {pid})...");
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .output();
    }

    if let Some(pid) = pid {
        let start = tokio::time::Instant::now();
        while process_exists(pid) && start.elapsed() < GRACEFUL_SHUTDOWN_TIMEOUT {
            sleep(GRACEFUL_SHUTDOWN_POLL_INTERVAL).await;
        }
    }

    if pid.is_some_and(process_exists) {
        tracing::warn!("Daemon did not exit gracefully, falling back to kill");
        stop_daemon().await?;
    } else {
        let _ = tokio::fs::remove_file(&pid_file).await;
        tracing::info!("Daemon shut down gracefully");
    }

    Ok(())
}

/// Restart the daemon (graceful stop + spawn).
pub async fn restart_daemon() -> Result<()> {
    tracing::info!("Restarting daemon...");
    graceful_shutdown().await?;

    // graceful_shutdown already waits up to 3s for the PID file to disappear.
    // Give a short extra grace period in case the old process is slow to exit.
    sleep(Duration::from_millis(200)).await;

    spawn_daemon_with_auto_exit(false).await?;
    tracing::info!("Daemon restarted successfully");
    Ok(())
}

/// Check daemon status.
pub async fn daemon_status() -> Result<String> {
    let addr = socket_addr();
    let pid_file = pid_file_path();

    if let Ok(stream) = kernel::transport::connect(&addr).await {
        drop(stream);
        tracing::info!("Daemon is running and accepting connections on {addr}");
        return Ok("Daemon is running".to_string());
    }

    let stale = cleanup_stale_pid_file().await;

    if stale {
        tracing::info!("Daemon is not running, cleaned stale PID file");
        Ok("Daemon is not running (stale PID cleaned)".to_string())
    } else if pid_file.exists() {
        tracing::info!("Daemon may be starting up (PID file exists but not responding yet)");
        Ok("Daemon may be starting up".to_string())
    } else {
        tracing::info!("Daemon is not running (no PID file, no socket)");
        Ok("Daemon is not running".to_string())
    }
}
