//! Workflows（`<data_dir>/workflows/`）：用户自有的可执行脚本库，
//! 经 channel slash 命令 `/workflow`（`/wkfl`）列出、执行、删除。
//!
//! 脚本约定：普通文件 + shebang + 可执行权限，直接 spawn（不经 shell，
//! shebang 由 OS 解析）；stdout/stderr 分管采集、按到达顺序并入同一缓冲
//! 保持时序。子进程注入 `YOMI_DATA_DIR`（与 shell 工具一致，脚本据此定位
//! yomi 资产），有会话上下文时另注入 `YOMI_SESSION_ID`。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use crate::types::{KernelError, Result};

// POSIX `setsid(2)` / `kill(2)`：与子进程同 linker 命名空间，unix 上始终
// 已链接，手动声明以避免 libc/nix 依赖（同 shell.rs 的做法）。
#[cfg(unix)]
extern "C" {
    fn setsid() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
const SIGKILL: i32 = 9;

/// 脚本库目录名（相对 `data_dir`）。
pub const DIR_NAME: &str = "workflows";

/// `run` 的执行上限：到期强杀，按超时上报。
pub const RUN_TIMEOUT: Duration = Duration::from_mins(5);

/// 主进程退出/被杀后 drain 收尾的宽限期：管道可能被脚本遗留的后台
/// 子进程继续持有（`sleep 30 &` 型），宽限到期放弃 drain，避免孤儿
/// 进程拖住回执。
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// 脚本库目录。
pub fn workflows_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR_NAME)
}

/// 一个 workflow 条目：文件名 + 是否带可执行位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowEntry {
    pub name: String,
    pub executable: bool,
}

/// 文件是否可执行（unix 看任一 exec 位；其他平台视为可执行）。
fn is_executable(md: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = md;
        true
    }
}

/// 列出脚本库：普通文件、跳过隐藏文件、按名排序；目录不存在视为空。
pub async fn list(data_dir: &Path) -> Result<Vec<WorkflowEntry>> {
    let mut rd = match tokio::fs::read_dir(workflows_dir(data_dir)).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut entries = Vec::new();
    while let Some(de) = rd.next_entry().await? {
        if !de.file_type().await?.is_file() {
            continue;
        }
        let Ok(name) = de.file_name().into_string() else {
            continue; // 非 UTF-8 文件名：无法经 slash 命令寻址，跳过
        };
        if name.starts_with('.') {
            continue;
        }
        entries.push(WorkflowEntry {
            name,
            executable: is_executable(&de.metadata().await?),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// 名字必须是裸文件名：非空、非点开头、无路径分隔符——既防路径穿越，
/// 也与 `list` 的可发现性一致（隐藏文件列不出来，就不给跑）。
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| !matches!(c, '/' | '\\' | '\0'));
    if ok {
        Ok(())
    } else {
        Err(KernelError::tool(format!(
            "invalid workflow name `{name}` — use a bare file name"
        )))
    }
}

/// 按名解析脚本路径：名字校验 + 存在 + 普通文件。
pub async fn resolve(data_dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    validate_name(name)?;
    let path = workflows_dir(data_dir).join(name);
    match tokio::fs::metadata(&path).await {
        Ok(md) if md.is_file() => Ok(Some(path)),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 脚本是否可执行（不存在即不可执行）。
pub async fn executable(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|md| md.is_file() && is_executable(&md))
}

/// 删除脚本；不存在返回 `false`。
pub async fn remove(data_dir: &Path, name: &str) -> Result<bool> {
    let Some(path) = resolve(data_dir, name).await? else {
        return Ok(false);
    };
    tokio::fs::remove_file(&path).await?;
    Ok(true)
}

/// 执行结果。
#[derive(Debug)]
pub struct RunOutcome {
    /// 退出码；被信号杀死为 `None`。
    pub exit_code: Option<i32>,
    /// 合并输出（stderr 按到达顺序与 stdout 并入）；超时时带上已产出部分。
    pub output: String,
    /// 是否因超时强杀。
    pub timed_out: bool,
    /// 实际执行耗时。
    pub elapsed: Duration,
}

/// 持续读空一根管道并入共享缓冲（按到达顺序，块粒度交错）。
async fn drain<R>(mut pipe: R, buf: std::sync::Arc<tokio::sync::Mutex<Vec<u8>>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.lock().await.extend_from_slice(&chunk[..n]),
        }
    }
}

/// 直接 spawn 脚本，在 `cwd` 下执行，`timeout` 到期 kill（超时仍返回
/// 已捕获的部分输出）。输出整体读入内存——脚本由管理员本人放置，
/// 面向 IM 回显的场景不会持续刷屏。
pub async fn run(
    path: &Path,
    args: &[String],
    cwd: &Path,
    data_dir: &Path,
    session_id: Option<&str>,
    timeout: Duration,
) -> Result<RunOutcome> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::utils::env::inject_child_env(&mut cmd, Some(data_dir), session_id);
    let started = std::time::Instant::now();
    // unix: 子进程独立成 session（setsid），超时/收尾时按进程组
    // （pgid == 子 pid）发 SIGKILL——只杀直接子进程会让 `sleep 60 &`
    // 型后裔继续持有管道，drain 被拖到地老天荒。
    #[cfg(unix)]
    unsafe {
        // SAFETY: `pre_exec` 在 fork 出的子进程中、exec 之前运行，只允许
        // async-signal-safe 操作；setsid 符合（同 shell.rs）。
        cmd.pre_exec(|| {
            if setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    // 两管各自持续读空：管道不排空，写多的脚本会阻塞在 write 上。
    let buf = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let mut drain_out = tokio::spawn(drain(
        child.stdout.take().expect("stdout piped"),
        std::sync::Arc::clone(&buf),
    ));
    let mut drain_err = tokio::spawn(drain(
        child.stderr.take().expect("stderr piped"),
        std::sync::Arc::clone(&buf),
    ));
    let wait = tokio::time::timeout(timeout, child.wait()).await;
    let (exit_code, timed_out) = match wait {
        Ok(Ok(status)) => (status.code(), false),
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => {
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    // 进程组仍在（wait 未返回），按组强杀，连后裔一起。
                    unsafe { kill(-(pid as i32), SIGKILL) };
                }
                let _ = child.wait().await; // 收割僵尸
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill().await;
            }
            (None, true)
        }
    };
    // 主进程已死：drain 宽限收尾，到期放弃（detach 前显式 abort）。
    let _ = tokio::join!(
        tokio::time::timeout(DRAIN_GRACE, &mut drain_out),
        tokio::time::timeout(DRAIN_GRACE, &mut drain_err),
    );
    drain_out.abort();
    drain_err.abort();
    let output = String::from_utf8_lossy(&buf.lock().await).to_string();
    Ok(RunOutcome {
        exit_code,
        output,
        timed_out,
        elapsed: started.elapsed(),
    })
}

#[cfg(test)]
#[path = "workflow_test.rs"]
mod tests;
