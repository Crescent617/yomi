//! Daemon lifecycle management for yomi.

use anyhow::{Context, Result};
pub use kernel::transport::{pid_file_path, socket_addr};
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

/// How long to wait for graceful shutdown before falling back to kill.
/// 必须罩住 kernel 侧关停预算（与 `kernel/mod.rs` 的 `stop_active_runs`
/// 互参）：等在跑 run 停完（60s 上界）+ 终态投递 grace（1.5s）+
/// persist drain（10s 上界）+ 连接排空（5s 上界）+ daemon hook 链
/// （`daemon_up` 在飞收尾 + `daemon_down` 全链，每条脚本 30s 上界）+
/// 进程退出余量。无在跑 run 时进程秒退，本上限只是病态工具的兜底。
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(90);
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

/// Hello 探测的单次上界：对面 accept 但一直不答 wire 协议时不能干等
/// RPC 超时（30s）。daemon 从 bind 到能答 hello 之间隔着 channel 初始化，
/// 可能很长——上界只兜"装死"的对面，不限制正常初始化时长（调用方按
/// 阶段给总预算，见 `spawn_daemon_with_auto_exit`）。
const HELLO_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Socket 可连 ≠ daemon 活着：unix socket 文件可能是残骸，端口对面
/// 可能 accept 但不是 yomi（或卡在初始化答不了 hello）。只有 wire
/// hello（协议版本校验）通过，才配说 "daemon already running"。
/// 探测用的连接立即丢弃（Drop 取消 reader/heartbeat）。
pub async fn try_connect_hello() -> Option<kernel::client::RemoteKernel> {
    tokio::time::timeout(
        HELLO_PROBE_TIMEOUT,
        kernel::client::RemoteKernel::connect(&socket_addr()),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
}

/// Spawn the daemon as a fully detached background process.
/// If a daemon is already accepting connections, returns Ok immediately.
/// Otherwise spawns a new process and polls until the daemon answers the
/// hello handshake (up to 30 s — channel init can be slow) so callers
/// never race with daemon initialisation.
pub async fn spawn_daemon() -> Result<()> {
    spawn_daemon_with_auto_exit(true).await
}

pub async fn spawn_daemon_with_auto_exit(auto_exit: bool) -> Result<()> {
    const SPAWN_READY_TIMEOUT: Duration = Duration::from_secs(30);
    const SPAWN_READY_INTERVAL: Duration = Duration::from_millis(100);

    // "已在跑"的判定必须过 hello：socket 残骸/非 yomi 监听不算。
    // hello 不通就继续走 spawn；真被占用会在 bind 或单例锁处得到
    // 明确错误。
    if try_connect_hello().await.is_some() {
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

    // Windows：无 console 的调用方（GUI/计划任务/Explorer）拉起 daemon 时，
    // CUI 子进程默认会被系统分配一个可见控制台窗口；与其它 spawn 点一样收口。
    kernel::utils::process::no_console_window_std(&mut cmd);

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

    // 就绪 = hello 握手通过（"socket 可 accept" 不算——调用方一拿到 Ok
    // 就开始发 RPC）。探测前先看子进程是否已退出：daemon start 是
    // 阻塞式前台进程，hello 只能等它初始化完才答得出；它活着但还没
    // 答 hello ≠ 失败。秒死由 try_wait 直接发现；总预算 30s 兜慢初始化；
    // 每个探测自身有 5s 上界，防对面装死。
    let start = tokio::time::Instant::now();
    let failure: Option<String> = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break Some(format!(
                    "daemon process (PID {pid}) exited before ready: {status}"
                ));
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("try_wait failed (PID {pid}): {e}"),
        }
        if try_connect_hello().await.is_some() {
            tracing::info!("Daemon ready after {:?}", start.elapsed());
            return Ok(());
        }
        if start.elapsed() >= SPAWN_READY_TIMEOUT {
            break Some(format!(
                "daemon spawned (PID {pid}) but did not become ready within {SPAWN_READY_TIMEOUT:?}"
            ));
        }
        sleep(SPAWN_READY_INTERVAL).await;
    };

    // 并发 spawn 竞态：我们的孩子可能因为抢锁/抢 socket 输给另一个
    // 同时启动的 daemon 而退出——只要 socket 后面答得出 hello，对方
    // 就是就绪的 daemon，视为成功（旧两段式行为一致）。
    if try_connect_hello().await.is_some() {
        tracing::info!(
            "Daemon ready after {:?} (spawned child lost the race)",
            start.elapsed()
        );
        return Ok(());
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
    let msg = failure.unwrap_or_else(|| "daemon failed to become ready".to_string());
    tracing::warn!("{msg}");
    Err(anyhow::anyhow!("{msg}"))
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
    /// Outer cap. Sized against the kernel's shutdown budget
    /// (`stop_active_runs`: up to 60s wind-down + 1.5s delivery grace +
    /// 10s persist drain + 5s connection drain ≈ 77s) plus respawn/ready
    /// slack — a slow wind-down must not fall back to the signal path.
    const WIRE_RESTART_TIMEOUT: Duration = Duration::from_secs(100);

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

    // graceful_shutdown already waits up to 90s for the PID file to disappear.
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
    // Same budget as WIRE_RESTART_TIMEOUT: a slow wind-down (up to ~77s
    // kernel-side) means the replacement may legitimately take that long
    // to own the socket.
    const SETTLE_GRACE: Duration = Duration::from_secs(90);
    const SETTLE_POLL: Duration = Duration::from_millis(200);

    let Some(old_pid) = old_pid else {
        return false;
    };
    let start = tokio::time::Instant::now();
    while start.elapsed() < SETTLE_GRACE {
        sleep(SETTLE_POLL).await;
        // 交接完成的判据 = 新 pid 持有 socket 且答 hello；只"能连"
        // 不算数（残骸 socket 也能连上 transport）。
        let pid = read_daemon_pid().await;
        if pid.is_some() && pid != Some(old_pid) && try_connect_hello().await.is_some() {
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

    // "Running" 的定义：wire hello 通过。transport 可连但答不了
    // hello 的（残骸 socket / 卡死的进程）不算 running，落到下面的
    // stale/starting 分支给处置提示。
    if let Some(kernel) = try_connect_hello().await {
        drop(kernel);
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
