//! spawn/ — 外挂执行引擎：hooks / tools 共用的子进程运行管线。
//!
//! 一次 spawn = 一次调用：stdin 喂字节、stdout/stderr 双管排空（开头
//! 与结尾各保留一半额度、超出丢中间，额度见 `spawn_captured_with_cap`
//! 的 `drain_cap`）、超时按进程树强杀（setsid/Job Object 由
//! [`crate::utils::process::spawn_in_new_tree`] 统一建立）、主进程死后
//! 双管共享 [`DRAIN_GRACE`] 宽限收尾。调用方只配 `Command` 的
//! program / cwd / env——stdio 与树管理由引擎接管。
//!
//! 故障分两层，调用方各自定策略（hook fail-open、tool fail-closed）：
//! - [`SpawnError`]：进程没起来 / wait 异常——引擎自身故障；
//! - [`Captured::timed_out`] / 非零 `exit_code`：外挂的回答。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncWriteExt as _;

use crate::utils::process::{kill_tree, spawn_in_new_tree};

/// drain 缓冲的默认累积上限：超出继续读（防管道阻塞）但停止累积——
/// 坏脚本 `cat hugefile >&2` 不会撑爆内存。
pub const DRAIN_CAP: usize = 64 * 1024;

/// 溢出落盘配置（sync shell 用）：某一流（stdout/stderr）累计超 cap
/// 时，把已捕获内容（此刻内存缓冲仍是完整流）一次性写入
/// `<dir>/<stem>_<stream>.log` 并开始流式追加——文件自第一字节完整；
/// 未超 cap 零 IO。unix 权限 0600：命令输出可能含敏感内容。
#[derive(Clone, Debug)]
pub struct OverflowLog {
    pub dir: std::path::PathBuf,
    pub stem: String,
}

impl OverflowLog {
    fn path_for(&self, stream: &str) -> std::path::PathBuf {
        self.dir.join(format!("{}_{stream}.log", self.stem))
    }
}

/// 主进程退出/被杀后 drain 收尾的宽限期。
pub const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// 一次 spawn 的捕获结果。
#[derive(Debug)]
pub struct Captured {
    /// 退出码；超时强杀（或信号终止）为 `None`。
    pub exit_code: Option<i32>,
    /// 终止信号编号（unix 以外恒 `None`）。
    pub signal: Option<i32>,
    /// 是否因超时被进程树强杀。
    pub timed_out: bool,
    /// 是否因取消被进程树强杀（与超时同路径，但语义分开：
    /// 调用方通常要把取消翻译成自己的取消语义而非"超时"）。
    pub cancelled: bool,
    /// stdout 捕获（≤ drain cap；用途由调用方决定）。
    pub stdout: Vec<u8>,
    /// stderr 捕获（≤ drain cap）。
    pub stderr: Vec<u8>,
    /// 溢出落盘的文件与字节数（配置了 [`OverflowLog`] 且对应流超 cap
    /// 时非空）；文件内容自第一字节完整。
    pub log_files: Vec<(std::path::PathBuf, u64)>,
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
    Exited(std::process::ExitStatus),
    WaitErr(std::io::Error),
    Timeout,
    Cancelled,
}

/// 运行一个命令并捕获其输出（drain 上限取默认值 [`DRAIN_CAP`]，
/// 不落盘）。
pub async fn spawn_captured(
    cmd: &mut tokio::process::Command,
    stdin_bytes: Option<&[u8]>,
    timeout: Duration,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<Captured, SpawnError> {
    spawn_captured_with_cap(cmd, stdin_bytes, timeout, cancel, DRAIN_CAP, None).await
}

/// 同 [`spawn_captured`]，但 drain 累积上限由调用方给定——shell 工具
/// 这类输出预算大的入口可以放宽，hooks 等用默认即可；`overflow` 配置
/// 后，超 cap 的流会全文落盘（见 [`OverflowLog`]）。
///
/// `cmd` 应已配好 program / cwd / env；stdio 由引擎接管（`stdin_bytes`
/// 为 `Some` 时管道写入，写遇 `BrokenPipe` 静默——脚本不读 stdin 是正常
/// 场景）。进程树由引擎统一建立，调用方不要再加 setsid。`cancel` 生效
/// 时与超时同路径按树强杀，返回 [`Captured::cancelled`]。
pub async fn spawn_captured_with_cap(
    cmd: &mut tokio::process::Command,
    stdin_bytes: Option<&[u8]>,
    timeout: Duration,
    cancel: Option<&tokio_util::sync::CancellationToken>,
    drain_cap: usize,
    overflow: Option<OverflowLog>,
) -> Result<Captured, SpawnError> {
    cmd.stdin(if stdin_bytes.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    let (mut child, tree) = spawn_in_new_tree(cmd).map_err(SpawnError::Spawn)?;
    // 两管各自持续读空（管道不排空，写多的脚本会阻塞）：状态共享——
    // 即使 drain 宽限到期被迫 abort，已捕获的部分仍读得到（后裔持有
    // 管道不见 EOF 的场景）。
    let out_state = Arc::new(tokio::sync::Mutex::new(StreamCapture::default()));
    let err_state = Arc::new(tokio::sync::Mutex::new(StreamCapture::default()));
    let mut drain_out = tokio::spawn(drain(
        child.stdout.take().expect("stdout piped"),
        Arc::clone(&out_state),
        drain_cap,
        overflow.as_ref().map(|o| o.path_for("stdout")),
    ));
    let mut drain_err = tokio::spawn(drain(
        child.stderr.take().expect("stderr piped"),
        Arc::clone(&err_state),
        drain_cap,
        overflow.as_ref().map(|o| o.path_for("stderr")),
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
                Ok(status) => Stop::Exited(status),
                Err(e) => Stop::WaitErr(e),
            },
            () = async { match cancel { Some(c) => c.cancelled().await, None => std::future::pending().await } } => Stop::Cancelled,
            () = tokio::time::sleep(timeout) => Stop::Timeout,
        }
    };
    let (exit_code, signal, timed_out, cancelled) = match stop {
        Stop::Exited(status) => {
            #[cfg(unix)]
            let signal = {
                use std::os::unix::process::ExitStatusExt as _;
                status.signal()
            };
            #[cfg(not(unix))]
            let signal = None;
            (status.code(), signal, false, false)
        }
        Stop::WaitErr(e) => {
            // wait 异常 = 子进程状态未知：按树尽力杀（与超时同路径），
            // 不留后裔。
            kill_tree(&mut child, &tree).await;
            if let Some(w) = &write {
                w.abort();
            }
            drain_out.abort();
            drain_err.abort();
            return Err(SpawnError::Wait(e));
        }
        Stop::Timeout | Stop::Cancelled => {
            let is_cancel = matches!(stop, Stop::Cancelled);
            kill_tree(&mut child, &tree).await;
            (None, None, !is_cancel, is_cancel)
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
    let out = std::mem::take(&mut *out_state.lock().await);
    let err = std::mem::take(&mut *err_state.lock().await);
    let StreamCapture {
        buf: out_buf,
        log: out_log,
        ..
    } = out;
    let StreamCapture {
        buf: err_buf,
        log: err_log,
        ..
    } = err;
    let mut log_files = Vec::new();
    for (log, path) in [
        (out_log, overflow.as_ref().map(|o| o.path_for("stdout"))),
        (err_log, overflow.as_ref().map(|o| o.path_for("stderr"))),
    ] {
        if let (Some(log), Some(path)) = (log, path) {
            if !log.failed {
                log_files.push((path, log.written));
            }
        }
    }
    Ok(Captured {
        exit_code,
        signal,
        timed_out,
        cancelled,
        stdout: out_buf.assemble(),
        stderr: err_buf.assemble(),
        log_files,
    })
}

/// drain 缓冲：开头与结尾各保留一半额度，超出后丢中间（不插标记，
/// 截断呈现由调用方负责）——洪泛输出的头（启动信息）与尾（错误行）
/// 通常都最有价值；无界缓冲则会被 `cat hugefile >&2` 型脚本撑爆内存。
#[derive(Default)]
struct DrainBuf {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
}

impl DrainBuf {
    fn push(&mut self, bytes: &[u8], half: usize) {
        let head_room = half.saturating_sub(self.head.len());
        let (to_head, rest) = bytes.split_at(head_room.min(bytes.len()));
        self.head.extend_from_slice(to_head);
        if !rest.is_empty() {
            self.tail.extend(rest);
            while self.tail.len() > half {
                self.tail.pop_front();
            }
        }
    }

    fn assemble(self) -> Vec<u8> {
        let mut out = self.head;
        out.extend(self.tail);
        out
    }
}

/// 单流捕获状态：内存缓冲 + 已读总量 + 溢出落盘文件。
#[derive(Default)]
struct StreamCapture {
    buf: DrainBuf,
    total: u64,
    log: Option<StreamLog>,
}

struct StreamLog {
    file: tokio::fs::File,
    written: u64,
    /// 落盘写失败（如磁盘满）：停止引用该文件——footer 声称「全文」
    /// 而文件不完整会误导后续排查。
    failed: bool,
}

/// 以 0600（unix）权限创建日志文件：命令输出可能含敏感内容。
/// spawn 引擎的溢出落盘与 shell 工具的 background 日志共用。
pub(crate) async fn open_log_file(path: &std::path::Path) -> std::io::Result<tokio::fs::File> {
    let mut opts = tokio::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    opts.open(path).await
}

/// 持续读空一根管道：内容入内存缓冲（[`DrainBuf`] 规则），首超 cap
/// 时把已捕获内容（此刻 head+tail 仍是完整流）落盘并开始流式追加，
/// 文件自第一字节完整。读不停：管道不排空，写多的脚本会阻塞在
/// write 上。
async fn drain<R>(
    mut pipe: R,
    state: Arc<tokio::sync::Mutex<StreamCapture>>,
    cap: usize,
    overflow_path: Option<std::path::PathBuf>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let half = cap / 2;
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut s = state.lock().await;
                let should_open =
                    s.log.is_none() && overflow_path.is_some() && s.total + n as u64 > cap as u64;
                if should_open {
                    let path = overflow_path.clone().unwrap_or_default();
                    match open_log_file(&path).await {
                        Ok(mut file) => {
                            let (t1, t2) = s.buf.tail.as_slices();
                            let mut written = 0u64;
                            let mut ok = true;
                            for part in [&s.buf.head[..], t1, t2] {
                                if file.write_all(part).await.is_err() {
                                    ok = false;
                                    break;
                                }
                                written += part.len() as u64;
                            }
                            if ok {
                                s.log = Some(StreamLog {
                                    file,
                                    written,
                                    failed: false,
                                });
                            }
                        }
                        Err(e) => {
                            tracing::debug!(path = %path.display(), error = %e, "overflow log create failed");
                        }
                    }
                }
                if let Some(log) = &mut s.log {
                    if log.failed || log.file.write_all(&chunk[..n]).await.is_err() {
                        log.failed = true;
                    } else {
                        log.written += n as u64;
                    }
                }
                s.buf.push(&chunk[..n], half);
                s.total += n as u64;
            }
        }
    }
}

#[cfg(all(test, unix))]
#[path = "spawn_test.rs"]
mod tests;
