//! spawn/ — 外挂执行引擎：hooks / tools 共用的子进程运行管线。
//!
//! 一次 spawn = 一次调用：stdin 喂字节、stdout/stderr 双管排空（各累积
//! 上限 [`DRAIN_CAP`]）、超时按进程组 SIGKILL（setsid 由引擎统一加，见
//! `utils::process::pre_exec_new_session`）、主进程死后双管共享
//! [`DRAIN_GRACE`] 宽限收尾。调用方只配 `Command` 的 program / cwd /
//! env——stdio 与 session 化由引擎接管。
//!
//! 故障分两层，调用方各自定策略（hook fail-open、tool fail-closed）：
//! - [`SpawnError`]：进程没起来 / wait 异常——引擎自身故障；
//! - [`Captured::timed_out`] / 非零 `exit_code`：外挂的回答。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;

// POSIX `kill(2)`：同原 hook/mod.rs 的做法，unix 上始终已链接，手动
// 声明以避免 libc/nix 依赖。setsid 收敛在 `utils::process`。
#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
const SIGKILL: i32 = 9;

/// drain 缓冲累积上限：超出继续读（防管道阻塞）但停止累积——坏脚本
/// `cat hugefile >&2` 不会撑爆内存。
pub const DRAIN_CAP: usize = 64 * 1024;

/// 主进程退出/被杀后 drain 收尾的宽限期。
pub const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// 一次 spawn 的捕获结果。
#[derive(Debug)]
pub struct Captured {
    /// 退出码；超时强杀（或信号终止）为 `None`。
    pub exit_code: Option<i32>,
    /// 是否因超时被进程组 SIGKILL。
    pub timed_out: bool,
    /// 是否因取消被进程组 SIGKILL（与超时同路径，但语义分开：
    /// 调用方通常要把取消翻译成自己的取消语义而非"超时"）。
    pub cancelled: bool,
    /// stdout 捕获（≤ [`DRAIN_CAP`]；用途由调用方决定）。
    pub stdout: Vec<u8>,
    /// stderr 捕获（≤ [`DRAIN_CAP`]）。
    pub stderr: Vec<u8>,
}

/// spawn 自身失败（与"外挂执行失败"分层）。
#[derive(Debug)]
pub enum SpawnError {
    /// 进程没起来（找不到文件、权限、shebang 坏……）。
    Spawn(std::io::Error),
    /// wait 异常。
    Wait(std::io::Error),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "spawn failed: {e}"),
            Self::Wait(e) => write!(f, "wait failed: {e}"),
        }
    }
}

impl std::error::Error for SpawnError {}

/// select 的归一支点：退出/超时/取消三个等待臂的统一返回型。
enum Stop {
    Exited(Option<i32>),
    WaitErr(std::io::Error),
    Timeout,
    Cancelled,
}

/// 运行一个命令并捕获其输出。
///
/// `cmd` 应已配好 program / cwd / env；stdio 由引擎接管（`stdin_bytes`
/// 为 `Some` 时管道写入，写遇 `BrokenPipe` 静默——脚本不读 stdin 是正常
/// 场景）。setsid 由引擎统一执行，调用方不要再加。`cancel` 生效时与
/// 超时同路径按进程组 SIGKILL，返回 [`Captured::cancelled`]。
pub async fn spawn_captured(
    cmd: &mut tokio::process::Command,
    stdin_bytes: Option<&[u8]>,
    timeout: Duration,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<Captured, SpawnError> {
    cmd.stdin(if stdin_bytes.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    let mut child = {
        crate::utils::process::pre_exec_new_session(cmd);
        cmd.spawn().map_err(SpawnError::Spawn)?
    };
    // 两管各自持续读空（管道不排空，写多的脚本会阻塞）：缓冲共享——
    // 即使 drain 宽限到期被迫 abort，已捕获的部分仍读得到（后裔持有
    // 管道不见 EOF 的场景）。
    let out_buf = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let err_buf = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let mut drain_out = tokio::spawn(drain(
        child.stdout.take().expect("stdout piped"),
        Arc::clone(&out_buf),
    ));
    let mut drain_err = tokio::spawn(drain(
        child.stderr.take().expect("stderr piped"),
        Arc::clone(&err_buf),
    ));
    // spawn 要求 'static：字节复制一份（memcpy 廉价；昂贵的序列化已在
    // 调用方按批只做一次）。
    let write = stdin_bytes.map(|bytes| {
        let mut stdin = child.stdin.take().expect("stdin piped");
        let bytes = bytes.to_vec();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt as _;
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
        })
    });
    let stop = {
        let wait = child.wait();
        tokio::pin!(wait);
        // biased：同时就绪按书写顺序归类——已退出 > 取消 > 超时；误归类
        // 会把已成功退出错报成超时（exit_code 随之丢失）。
        tokio::select! {
            biased;
            r = &mut wait => match r {
                Ok(status) => Stop::Exited(status.code()),
                Err(e) => Stop::WaitErr(e),
            },
            () = async { match cancel { Some(c) => c.cancelled().await, None => std::future::pending().await } } => Stop::Cancelled,
            () = tokio::time::sleep(timeout) => Stop::Timeout,
        }
    };
    let (exit_code, timed_out, cancelled) = match stop {
        Stop::Exited(code) => (code, false, false),
        Stop::WaitErr(e) => {
            // wait 异常 = 子进程状态未知：按组尽力杀（与超时同路径），
            // 不留后裔。
            kill_tree(&mut child).await;
            if let Some(w) = &write {
                w.abort();
            }
            drain_out.abort();
            drain_err.abort();
            return Err(SpawnError::Wait(e));
        }
        Stop::Timeout | Stop::Cancelled => {
            let is_cancel = matches!(stop, Stop::Cancelled);
            kill_tree(&mut child).await;
            (None, !is_cancel, is_cancel)
        }
    };
    if let Some(w) = &write {
        w.abort();
    }
    // 主进程已死：两管共享一段 drain 宽限收尾，到期放弃（detach 前显式
    // abort）；缓冲是共享的，abort 后已捕获内容仍在。
    let _ = tokio::join!(
        tokio::time::timeout(DRAIN_GRACE, &mut drain_out),
        tokio::time::timeout(DRAIN_GRACE, &mut drain_err),
    );
    drain_out.abort();
    drain_err.abort();
    let stdout = std::mem::take(&mut *out_buf.lock().await);
    let stderr = std::mem::take(&mut *err_buf.lock().await);
    Ok(Captured {
        exit_code,
        timed_out,
        cancelled,
        stdout,
        stderr,
    })
}

/// 尽力杀整棵树：unix 按进程组 SIGKILL（连后裔；组不存在或已退出则
/// 静默 ESRCH）并收割僵尸；非 unix 杀主进程。
async fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            unsafe { kill(-(pid as i32), SIGKILL) };
        }
        let _ = child.wait().await;
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }
}

/// 持续读空一根管道并入共享缓冲；累积到 [`DRAIN_CAP`] 后继续读但停止
/// 累积（防管道阻塞的同时防内存放大）。
async fn drain<R>(mut pipe: R, buf: Arc<tokio::sync::Mutex<Vec<u8>>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut b = buf.lock().await;
                let room = DRAIN_CAP.saturating_sub(b.len());
                if room > 0 {
                    b.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "spawn_test.rs"]
mod tests;
