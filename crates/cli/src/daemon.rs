//! Daemon lifecycle management for yomi.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::net::UnixStream;
use tokio::time::{sleep, Duration};

/// How long to wait for graceful shutdown before falling back to kill.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
/// Polling interval while waiting for graceful shutdown.
const GRACEFUL_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Max retries when waiting for socket disappearance during restart.
const RESTART_MAX_RETRIES: usize = 30;
/// Polling interval while waiting for socket disappearance during restart.
const RESTART_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Get the Unix socket path for the daemon.
///
/// Resolution order:
/// 1. `YOMI_SOCKET` env var (highest priority)
/// 2. `XDG_RUNTIME_DIR` env var
/// 3. Platform-specific data dir
/// 4. `/tmp` fallback
pub fn socket_path() -> PathBuf {
    let socket_env = format!("{}SOCKET", kernel::ENV_PREFIX);
    if let Some(path) = std::env::var_os(&socket_env) {
        return PathBuf::from(path);
    }
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
        || {
            directories::BaseDirs::new().map_or_else(
                || PathBuf::from("/tmp/yomi-daemon.sock"),
                |b| b.data_dir().join("yomi/daemon.sock"),
            )
        },
        |p| PathBuf::from(p).join("yomi/daemon.sock"),
    )
}

/// Returns the PID file path used for daemon process tracking.
///
/// Defaults to `<socket_path>.pid`.
fn pid_file_path() -> PathBuf {
    let mut p = socket_path();
    p.set_extension("pid");
    p
}

/// Check whether a process with the given PID exists.
///
/// Uses `kill(pid, 0)` which works on all POSIX systems (Linux, macOS, *BSD).
/// No signal is actually sent; the syscall only checks permissions / process existence.
#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        None, // kill(pid, 0)
    )
    .is_ok()
}

#[cfg(not(unix))]
fn process_exists(_pid: u32) -> bool {
    // Conservative: assume the process exists on non-Unix platforms so we
    // never delete a socket that might belong to a running daemon.
    true
}

/// Try connecting to the daemon socket.
pub async fn try_connect() -> Option<UnixStream> {
    let sock = socket_path();
    if !sock.exists() {
        return None;
    }
    match UnixStream::connect(&sock).await {
        Ok(stream) => Some(stream),
        Err(_) => {
            // Be conservative: only delete the socket if we can prove the
            // owner is dead (PID file exists but PID is gone).  If the PID
            // file is missing we assume the daemon is still starting up.
            let pid_file = pid_file_path();
            if pid_file.exists() {
                if let Ok(s) = tokio::fs::read_to_string(&pid_file).await {
                    if let Ok(pid) = s.trim().parse::<u32>() {
                        if !process_exists(pid) {
                            let _ = tokio::fs::remove_file(&sock).await;
                            let _ = tokio::fs::remove_file(&pid_file).await;
                        }
                    }
                }
            }
            None
        }
    }
}

/// Spawn the daemon as a fully detached background process.
/// Returns Ok(()) immediately if a daemon is already running.
pub async fn spawn_daemon() -> Result<()> {
    let sock = socket_path();
    let pid_file = pid_file_path();

    // Check if a daemon is already running before spawning.
    if try_connect().await.is_some() {
        tracing::info!("Daemon already running, skipping spawn");
        return Ok(());
    }

    if let Some(parent) = sock.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let _ = tokio::fs::remove_file(&sock).await;
    let _ = tokio::fs::remove_file(&pid_file).await;

    let current_exe = std::env::current_exe().context("Failed to get current executable")?;

    // Use std::process::Command for better control over process spawning.
    let mut cmd = std::process::Command::new(&current_exe);
    cmd.arg("daemon")
        .arg("start")
        .arg("--auto-exit")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Create new session: daemon becomes session & process group leader,
        // fully detaching from the controlling terminal so SIGHUP on terminal
        // close does not reach it.
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid().map_err(std::io::Error::other)?;
                Ok(())
            });
        }
    }

    let child = cmd.spawn().context("Failed to spawn daemon process")?;

    let pid = child.id();
    tokio::fs::write(&pid_file, pid.to_string()).await?;

    tracing::info!("Spawned daemon with PID {pid}");
    Ok(())
}

/// Force-stop the daemon by sending SIGKILL and removing socket/pid files.
/// This is a **last-resort fallback** when graceful shutdown fails.
pub async fn stop_daemon() -> Result<()> {
    let sock = socket_path();
    let pid_file = pid_file_path();

    if let Ok(pid_str) = tokio::fs::read_to_string(&pid_file).await {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .output();
            }
            #[cfg(not(unix))]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            }
        }
    }

    let _ = tokio::fs::remove_file(&sock).await;
    let _ = tokio::fs::remove_file(&pid_file).await;
    Ok(())
}

/// Gracefully shut down the daemon by sending SIGTERM.
/// Falls back to `stop_daemon` (SIGKILL) if the daemon does not exit.
pub async fn graceful_shutdown() -> Result<()> {
    let sock = socket_path();
    let pid_file = pid_file_path();

    if !sock.exists() && !pid_file.exists() {
        tracing::info!("No daemon found, nothing to stop");
        return Ok(());
    }

    let pid = match tokio::fs::read_to_string(&pid_file).await {
        Ok(s) => s.trim().parse::<i32>().ok(),
        Err(_) => None,
    };

    #[cfg(unix)]
    if let Some(pid) = pid {
        tracing::info!("Sending SIGTERM to daemon (PID {pid})...");
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        );

        // Wait up to a bounded timeout for the socket to disappear.
        let _ = tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, async {
            while sock.exists() {
                sleep(GRACEFUL_SHUTDOWN_POLL_INTERVAL).await;
            }
        })
        .await;
    }

    if sock.exists() || pid_file.exists() {
        tracing::warn!("Daemon did not exit gracefully, falling back to kill");
        stop_daemon().await?;
    } else {
        tracing::info!("Daemon shut down gracefully");
    }

    Ok(())
}

/// Restart the daemon (graceful stop + spawn).
pub async fn restart_daemon() -> Result<()> {
    graceful_shutdown().await?;

    // Wait for both socket and PID to disappear before spawning.
    // This prevents a double-daemon scenario when the old process
    // takes longer than the graceful-shutdown timeout to exit.
    let sock = socket_path();
    let pid_file = pid_file_path();
    for _ in 0..RESTART_MAX_RETRIES {
        let socket_gone = !sock.exists();
        let pid_gone = if pid_file.exists() {
            match tokio::fs::read_to_string(&pid_file).await {
                Ok(s) => s
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .is_none_or(|pid| !process_exists(pid)),
                Err(_) => true,
            }
        } else {
            true
        };
        if socket_gone && pid_gone {
            break;
        }
        sleep(RESTART_POLL_INTERVAL).await;
    }

    spawn_daemon().await
}

/// Check daemon status.
pub async fn daemon_status() -> Result<String> {
    let sock = socket_path();
    let pid_file = pid_file_path();

    if !sock.exists() {
        return Ok("Daemon is not running".to_string());
    }

    // If we can connect, it's alive.
    if UnixStream::connect(&sock).await.is_ok() {
        return Ok("Daemon is running".to_string());
    }

    // Can't connect — check PID file to decide if socket is stale.
    let stale = if pid_file.exists() {
        match tokio::fs::read_to_string(&pid_file).await {
            Ok(s) => s
                .trim()
                .parse::<u32>()
                .ok()
                .is_none_or(|pid| !process_exists(pid)),
            Err(_) => true,
        }
    } else {
        // PID file missing but socket exists: conservative — don't delete,
        // the daemon may just be starting up.
        false
    };

    if stale {
        let _ = tokio::fs::remove_file(&sock).await;
        let _ = tokio::fs::remove_file(&pid_file).await;
        Ok("Daemon is not running (stale socket cleaned)".to_string())
    } else {
        Ok("Socket exists but daemon may be starting up".to_string())
    }
}
