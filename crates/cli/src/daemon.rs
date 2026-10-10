//! Daemon lifecycle management for yomi.

use anyhow::{Context, Result};
#[cfg(not(unix))]
pub use kernel::transport::pid_file_path;
pub use kernel::transport::socket_addr;
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

/// Wait until nothing answers on the daemon socket — i.e. the process has
/// exited and released the port. On Windows this is the only hard exit
/// criterion: no flock, no reliable pid liveness, so the wire is the
/// ground truth (same principle as the unix lock-oracle). Returns true
/// if the socket went silent within `timeout`.
#[cfg(not(unix))]
async fn wait_until_down(timeout: Duration, interval: Duration) -> bool {
    let start = tokio::time::Instant::now();
    loop {
        if try_connect().await.is_none() {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        sleep(interval).await;
    }
}

/// Try connecting to the daemon.
pub async fn try_connect() -> Option<kernel::transport::Stream> {
    let addr = socket_addr();
    kernel::transport::connect(&addr).await.ok()
}

/// 当前环境对应的 `data_dir`：与 `init_kernel` 同款推导（config 文件
/// 发现 + env 覆盖 + finalize）。锁探针的键——只按 env 推会让
/// config.toml 里设了 `data_dir` 的用户探错锁、`daemon stop` 静默空转。
#[cfg(unix)]
fn data_dir() -> Result<PathBuf> {
    Ok(crate::utils::load_config(None)?.data_dir)
}

/// 锁探针结果：`data_dir` 有没有 cron daemon 持有。
#[cfg(unix)]
enum LockProbe {
    /// 锁空闲：没有 cron daemon 在跑（探针守卫已即取即放）。
    Free,
    /// 锁被持有。owner 为 best-effort 读到的持有者信息，可能 None。
    Held(Option<kernel::daemon_lock::LockOwner>),
}

/// 锁探针：非阻塞试抢 `data_dir` 单例锁。
///
/// 这是比 pid 文件更硬的活性/身份判据：flock 被持有 ⇒ 持有者进程
/// 存活 ⇒ 它的 pid 不可能被回收 ⇒ meta 里的 pid 就是持有者本人。pid
/// 文件做不到——跨 pod 残留的 pid 在新 pid namespace 里早已易主，
/// 拿它发信号会误杀无关进程。
#[cfg(unix)]
fn probe_lock() -> Result<LockProbe> {
    let data_dir = data_dir()?;
    match kernel::daemon_lock::acquire(&data_dir) {
        Ok(guard) => {
            drop(guard);
            Ok(LockProbe::Free)
        }
        Err(kernel::daemon_lock::AcquireError::Contended { owner }) => Ok(LockProbe::Held(owner)),
        Err(kernel::daemon_lock::AcquireError::Io(e)) => Err(e.into()),
    }
}

/// 锁持有者的 pid。只在锁被持有的前提下可信（见 `probe_lock`）；
/// 锁空闲时 meta 是旧残留，读出来的 pid 可能已经易主。
#[cfg(unix)]
fn lock_holder_pid() -> Option<u32> {
    data_dir()
        .ok()
        .and_then(|d| kernel::daemon_lock::read_owner(&d))
        .map(|o| o.pid)
}

/// 停机信号的发送对象。
///
/// 以 `data_dir` 锁为键：停机找的是"这个 `data_dir` 的 cron 持有者"，
/// 不是"这个 socket 后面的进程"。socket override 的调用方必须同时
/// 给出 `YOMI_DATA_DIR`，否则探到的是默认 `data_dir` 的锁。
#[cfg(unix)]
enum StopTarget {
    /// 没有 cron daemon 在跑，无需停机。
    Nothing,
    /// 持有者 pid，可发信号。
    Pid(u32),
}

/// 解析停机信号目标。锁不可信信息（持有者 meta 不可读）时给出可
/// 操作的报错——宁可拒发信号，也不盲杀。
#[cfg(unix)]
fn stop_target() -> Result<StopTarget> {
    match probe_lock()? {
        LockProbe::Free => Ok(StopTarget::Nothing),
        LockProbe::Held(Some(owner)) => Ok(StopTarget::Pid(owner.pid)),
        LockProbe::Held(None) => {
            let dir = data_dir()?;
            anyhow::bail!(
                "单例锁被持有但持有者信息不可读（{}），无法定位进程，未发信号以避免误杀；\
                 请确认占用者后手动处理",
                kernel::daemon_lock::lock_file_path(&dir).display()
            )
        }
    }
}

/// 等锁释放（= 持有者退出：进程死则内核释放 flock），超时返回 false。
#[cfg(unix)]
async fn wait_lock_free(timeout: Duration, interval: Duration) -> bool {
    let start = tokio::time::Instant::now();
    loop {
        match probe_lock() {
            Ok(LockProbe::Free) => return true,
            Ok(LockProbe::Held(_)) => {}
            Err(e) => tracing::warn!("lock probe failed: {e}"),
        }
        if start.elapsed() >= timeout {
            return false;
        }
        sleep(interval).await;
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
#[cfg(unix)]
pub async fn stop_daemon() -> Result<()> {
    let pid = match stop_target()? {
        StopTarget::Nothing => {
            tracing::info!("No daemon found, nothing to stop");
            return Ok(());
        }
        StopTarget::Pid(pid) => pid,
    };

    tracing::info!("Sending SIGKILL to daemon (PID {pid})...");
    if let Err(e) = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    ) {
        // 探针与信号之间持有者可能已完成自行停机：进程已不在（ESRCH）
        // 但锁随之释放，这是成功收尾而非失败——交由下面的锁等待确认。
        if e != nix::errno::Errno::ESRCH {
            anyhow::bail!("failed to kill daemon process {pid}: {e}");
        }
    }

    // 等锁释放 = 持有者真死了（进程死则内核释放 flock），比轮询
    // pid 活性硬：跨 pid namespace 也成立。
    if !wait_lock_free(Duration::from_secs(2), Duration::from_millis(50)).await {
        anyhow::bail!("daemon process {pid} is still holding the lock after SIGKILL");
    }
    tracing::info!("Daemon force-stopped");
    Ok(())
}

/// Force-stop the daemon and wait for the process to actually exit.
#[cfg(not(unix))]
pub async fn stop_daemon() -> Result<()> {
    let pid_file = pid_file_path();
    let pid = match tokio::fs::read_to_string(&pid_file).await {
        Ok(s) => s.trim().parse::<u32>().ok(),
        Err(_) => None,
    };

    if let Some(pid) = pid {
        tracing::info!("Sending kill signal to daemon (PID {pid})...");
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
    }

    // 等 socket 不再应答 = 进程真退了、端口已释放，后续 spawn 不会与
    // 旧进程撞端口。pid 文件缺失/指向已死进程时这里立即返回成功。
    if !wait_until_down(Duration::from_secs(5), Duration::from_millis(100)).await {
        anyhow::bail!("daemon is still answering on {} after kill", socket_addr());
    }

    // Only remove PID file after confirming the daemon is gone.
    let _ = tokio::fs::remove_file(&pid_file).await;

    tracing::info!("Daemon force-stopped");
    Ok(())
}

/// Gracefully shut down the daemon.
/// Falls back to `stop_daemon` if the daemon does not exit.
#[cfg(unix)]
pub async fn graceful_shutdown() -> Result<()> {
    let pid = match stop_target()? {
        StopTarget::Nothing => {
            tracing::info!("No daemon found, nothing to stop");
            return Ok(());
        }
        StopTarget::Pid(pid) => pid,
    };

    tracing::info!("Sending SIGTERM to daemon (PID {pid})...");
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGTERM,
    );

    if wait_lock_free(GRACEFUL_SHUTDOWN_TIMEOUT, GRACEFUL_SHUTDOWN_POLL_INTERVAL).await {
        tracing::info!("Daemon shut down gracefully");
        Ok(())
    } else {
        tracing::warn!("Daemon did not exit gracefully, falling back to kill");
        stop_daemon().await
    }
}

/// Gracefully shut down the daemon.
/// Falls back to `stop_daemon` if the daemon does not exit.
#[cfg(not(unix))]
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

    if let Some(pid) = pid {
        tracing::info!("Sending graceful shutdown to daemon (PID {pid})...");
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .output();
    }

    // Windows 锁是 no-op，"进程退了没"只能看 socket 还答不答（同 unix
    // 锁探针一个原则：活性/退出判据必须硬）。优雅窗口走完仍应答才升级
    // 强杀；pid 文件缺失/指向已死进程时 taskkill 无效、等待立即成功。
    if wait_until_down(GRACEFUL_SHUTDOWN_TIMEOUT, GRACEFUL_SHUTDOWN_POLL_INTERVAL).await {
        let _ = tokio::fs::remove_file(&pid_file).await;
        tracing::info!("Daemon shut down gracefully");
        Ok(())
    } else {
        tracing::warn!("Daemon did not exit gracefully, falling back to kill");
        stop_daemon().await
    }
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
    let old_pid = current_holder_pid().await;
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

    // graceful_shutdown already waits up to 90s for the lock to release.
    // Give a short extra grace period in case the old process is slow to exit.
    sleep(Duration::from_millis(200)).await;

    spawn_daemon_with_auto_exit(false).await?;
    tracing::info!("Daemon restarted successfully (signal)");
    Ok(())
}

/// 当前 daemon 持有者的 pid。unix 从锁 meta 读——只在锁被持有的前提
/// 下可信（flock 被持有 ⇒ 持有者存活 ⇒ pid 未回收），restart 交接判定
/// 正是在持有语境下用；Windows 锁是 no-op，退回 pid 文件。
async fn current_holder_pid() -> Option<u32> {
    #[cfg(unix)]
    {
        lock_holder_pid()
    }
    #[cfg(not(unix))]
    {
        tokio::fs::read_to_string(pid_file_path())
            .await
            .ok()
            .and_then(|s| s.trim().parse().ok())
    }
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
        // 交接完成的判据 = 锁换了持有者（meta pid 变化）且答 hello；
        // 只"能连"不算数（残骸 socket 也能连上 transport）。
        let pid = current_holder_pid().await;
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
///
/// 只依赖 socket（与 `socket_addr()` 同键）：hello 通 = running；不通
/// 时看 socket 文件还在不在（unix）——在 = 可能在初始化/关停中，不在 =
/// 没有 daemon。故意不用锁探针：status 常被只带 `YOMI_SOCKET` 的调用
/// 使用，那时推导出的 `data_dir` 与 socket 背后的 daemon 可能无关，
/// 探别人的锁既不准确也侵入。
pub async fn daemon_status() -> Result<String> {
    let addr = socket_addr();
    // "Running" 的定义：wire hello 通过。transport 可连但答不了
    // hello 的（残骸 socket / 卡死的进程）不算 running。
    if let Some(kernel) = try_connect_hello().await {
        drop(kernel);
        tracing::info!("Daemon is running and accepting connections on {addr}");
        return Ok("Daemon is running".to_string());
    }
    let socket_file_exists = match &addr {
        kernel::transport::SocketAddr::Unix(p) => p.exists(),
        _ => false,
    };
    if socket_file_exists {
        tracing::info!("Daemon may be starting up (socket file exists but not responding yet)");
        Ok("Daemon may be starting up".to_string())
    } else {
        tracing::info!("Daemon is not running (no hello, no socket file)");
        Ok("Daemon is not running".to_string())
    }
}

#[cfg(test)]
#[path = "daemon_test.rs"]
mod tests;
