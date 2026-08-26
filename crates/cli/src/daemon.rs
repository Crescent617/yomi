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

/// Restart the daemon.
///
/// Prefers the in-band wire restart ([`kernel::client::KernelApi::restart`]):
/// the daemon then spawns its own replacement, so the new process inherits
/// the *daemon's* environment and its original `--auto-exit` setting — not
/// this CLI caller's environment (which for agent shell calls would be a
/// stripped tool env). Falls back to the signal-based path when the daemon
/// is unreachable, rejects the request, or the replacement never comes up.
pub async fn restart_daemon() -> Result<()> {
    /// Outer cap; the client itself polls for the replacement for up to
    /// `CONNECT_RETRY_TIMEOUT` (10 s), so this only guards against hangs.
    const WIRE_RESTART_TIMEOUT: Duration = Duration::from_secs(20);

    tracing::info!("Restarting daemon...");
    let old_pid = read_daemon_pid().await;
    let wire_result: Result<()> = match connect_strict().await {
        Ok(kernel) => {
            match tokio::time::timeout(
                WIRE_RESTART_TIMEOUT,
                kernel::client::KernelApi::restart(&kernel),
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                // The daemon DID come back, but the saved config could not
                // be applied — that is a config problem to surface, never
                // a reason to kill the fresh daemon via the signal path.
                Ok(Err(e)) if is_config_not_applied(&e) => {
                    return Err(anyhow::anyhow!("{e}"));
                }
                Ok(Err(e)) => Err(anyhow::anyhow!("wire restart rejected: {e}")),
                Err(_) => Err(anyhow::anyhow!("wire restart timed out")),
            }
        }
        Err(e) => Err(e),
    };
    match wire_result {
        Ok(()) => {
            tracing::info!("Daemon restarted successfully (wire)");
            return Ok(());
        }
        Err(e) => {
            tracing::warn!("wire restart unavailable ({e}); verifying before signal fallback");
            if self_restart_settled(old_pid).await {
                tracing::info!("Daemon restarted successfully (wire, settled during grace)");
                return Ok(());
            }
            tracing::warn!("falling back to signal-based restart");
        }
    }

    graceful_shutdown().await?;

    // graceful_shutdown already waits up to 3s for the PID file to disappear.
    // Give a short extra grace period in case the old process is slow to exit.
    sleep(Duration::from_millis(200)).await;

    spawn_daemon_with_auto_exit(false).await?;
    tracing::info!("Daemon restarted successfully (signal)");
    Ok(())
}

/// Read the daemon's pid file (missing/invalid → None).
async fn read_daemon_pid() -> Option<u32> {
    tokio::fs::read_to_string(pid_file_path())
        .await
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// `KernelApi::restart` 的"已重启但配置未生效"错误判定。`KernelError`
/// 的 Display 带变体前缀（如 `"Configuration error: "`），不能拿
/// `to_string()` 与消息常量裸比，必须结构匹配。
fn is_config_not_applied(e: &kernel::types::KernelError) -> bool {
    matches!(
        e,
        kernel::types::KernelError::Config(msg) if msg == kernel::client::RESTART_CONFIG_NOT_APPLIED
    )
}

/// Grace poll after a failed/timed-out wire restart: the daemon may have
/// accepted the request and be mid-self-restart — its drain plus respawn
/// can outlast our outer timeout. If a *different* pid comes to own the
/// socket, the restart already happened; never SIGTERM that fresh daemon.
async fn self_restart_settled(old_pid: Option<u32>) -> bool {
    const SETTLE_GRACE: Duration = Duration::from_secs(10);
    const SETTLE_POLL: Duration = Duration::from_millis(200);

    let Some(old_pid) = old_pid else {
        return false;
    };
    let start = tokio::time::Instant::now();
    while start.elapsed() < SETTLE_GRACE {
        sleep(SETTLE_POLL).await;
        let pid = read_daemon_pid().await;
        if pid.is_some() && pid != Some(old_pid) && try_connect().await.is_some() {
            return true;
        }
    }
    false
}

/// Connect to a running daemon with a strict hello handshake.
///
/// Shared by the daemon-only commands (session/cron/events/rpc): unlike
/// `select_kernel` this never spawns and never falls back to a local
/// kernel — the daemon must be up and protocol-compatible.
pub async fn connect_strict() -> Result<kernel::client::RemoteKernel> {
    kernel::client::RemoteKernel::connect(&socket_addr())
        .await
        .context("Failed to connect to daemon. Is it running?")
}

/// Kernel selection shared by `run` and `tui` (driven by their
/// `--bg` / `--fg` flags, see `KernelModeArgs`):
///
/// - `--fg`: local in-process kernel, the daemon is left untouched.
/// - `--bg`: background daemon mode, spawning it when needed; the connection
///   must pass the hello handshake — strict, no fallback.
/// - neither (auto): use a running daemon that passes hello; fall back to
///   local only when no daemon is running at all. A daemon that accepts the
///   socket but fails hello is a hard error — never a silent local fallback.
///
/// Returns the kernel plus whether it is daemon-backed.
pub async fn select_kernel(
    mode: &crate::args::KernelModeArgs,
    config: &kernel::config::Config,
) -> Result<(std::sync::Arc<dyn kernel::client::KernelApi>, bool)> {
    use kernel::client::RemoteKernel;
    use std::sync::Arc;

    if mode.fg {
        tracing::info!("--fg: using local in-process kernel");
        return Ok((
            crate::commands::tui::create_local_kernel(config).await?,
            false,
        ));
    }

    if mode.bg {
        tracing::info!("--bg: using daemon");
        spawn_daemon().await?;
        let kernel = RemoteKernel::connect(&socket_addr())
            .await
            .context("Daemon failed the hello handshake")?;
        return Ok((Arc::new(kernel), true));
    }

    if try_connect().await.is_none() {
        tracing::info!("No running daemon; using local in-process kernel");
        return Ok((
            crate::commands::tui::create_local_kernel(config).await?,
            false,
        ));
    }
    let kernel = RemoteKernel::connect(&socket_addr()).await.map_err(|e| {
        anyhow::anyhow!(
            "A daemon is running but failed the hello handshake ({e}); \
             refusing to fall back to a local kernel. \
             Fix it with `yomi daemon restart`, or use `--fg`."
        )
    })?;
    tracing::info!("Using running daemon");
    Ok((Arc::new(kernel), true))
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

#[cfg(test)]
#[path = "daemon_test.rs"]
mod tests;
