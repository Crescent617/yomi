//! 子进程树生命周期管理：spawn 收口与整体收尾。
//!
//! 所有需要「超时/取消时把整棵树收干净」的 spawn 点统一走
//! [`spawn_in_new_tree`] + [`kill_tree`]；只知 pid 的外部管理面
//! （后台任务 tracker）走 [`terminate_tree_by_pid`]。平台各一个正解：
//!
//! - unix：`setsid` 让子进程成为新进程组组长（pgid == 子 pid），
//!   按组发信号，连 `sleep 60 &` 型后裔一起收——只杀直接子进程会让
//!   后裔继续持有管道/资源；
//! - windows：子进程挂进配置了 `KILL_ON_JOB_CLOSE` 的 Job Object，
//!   [`kill_tree`] 走 `TerminateJobObject`；[`ProcessTree`] drop 关闭
//!   句柄时内核回收 job 内残余进程，daemon 退出同理（句柄被系统收
//!   走）。job 创建/挂载失败时降级为只杀主进程（日志 debug）。
//!
//! 平台语义差异（有意为之）：windows 上命令结束后残留的后裔会被
//! job 回收；unix 上按现状保留（session 不归我们管）。

use std::io;

// POSIX `setsid(2)`/`kill(2)`：与 libc 同 linker 命名空间，unix 上
// 始终已链接，手动声明以避免 libc/nix 依赖。
#[cfg(unix)]
extern "C" {
    fn setsid() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
const SIGKILL: i32 = 9;
#[cfg(unix)]
const SIGTERM: i32 = 15;

#[cfg(windows)]
mod windows_job;

/// 让子进程独立成新 session（`setsid`）：子进程成为新进程组的组长
/// （pgid == 子 pid），超时/收尾时按组发信号能连后裔一起收。unix
/// 之外为 no-op。
///
/// 一般不需要直接调用：[`spawn_in_new_tree`] 已在 spawn 前统一处理。
/// 返回 `&mut` 便于链式（同 `utils::env::inject_child_env`）。
pub fn pre_exec_new_session(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(unix)]
    unsafe {
        // SAFETY: `pre_exec` 在 fork 出的子进程中、exec 之前运行，只允许
        // async-signal-safe 操作；setsid 符合。
        cmd.pre_exec(|| {
            if setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd
}

/// 一次 spawn 的收尾凭据：unix 为进程组 id（== 子进程 pid），windows
/// 为 Job Object（`None` = 挂载失败，kill 降级为只杀主进程）。
pub struct ProcessTree {
    #[cfg(unix)]
    pgid: i32,
    #[cfg(windows)]
    job: Option<windows_job::JobObject>,
}

/// spawn 一个子进程并建立整体收尾能力（见模块文档）。
///
/// 调用方负责 `kill_on_drop` 与 stdio；本函数只管「树」的建立。
pub fn spawn_in_new_tree(
    cmd: &mut tokio::process::Command,
) -> io::Result<(tokio::process::Child, ProcessTree)> {
    #[cfg(unix)]
    {
        pre_exec_new_session(cmd);
        let child = cmd.spawn()?;
        let tree = ProcessTree {
            pgid: child.id().map_or(0, |pid| pid as i32),
        };
        Ok((child, tree))
    }
    #[cfg(windows)]
    {
        // 先 spawn 再 assign：CREATE_SUSPENDED 路线需要主线程句柄才能
        // resume，std/tokio 均不暴露；spawn 与 assign 之间微秒级的窗口
        // 里子进程建出的后裔不进 job，可接受。tokio 的 Child 在 Windows
        // 不暴露进程句柄，按 pid 自行 OpenProcess（子进程在此间已退出
        // 则打开失败，同样降级）。
        let child = cmd.spawn()?;
        let job = child.id().and_then(|pid| {
            match windows_job::ProcessHandle::open_for_job_assign(pid).and_then(|proc| {
                let job = windows_job::JobObject::new_kill_on_close()?;
                // SAFETY: proc 是有效的进程句柄（存活于本闭包期间），
                // job 挂载后内核自持进程引用，proc 句柄随闭包结束关闭。
                unsafe { job.assign_raw(proc.raw()) }?;
                Ok(job)
            }) {
                Ok(job) => Some(job),
                Err(e) => {
                    tracing::debug!(error = %e, "job object unavailable; tree-kill degraded to main process");
                    None
                }
            }
        });
        Ok((child, ProcessTree { job }))
    }
}

/// 强杀整棵树并收割主进程：unix 按进程组 SIGKILL（组不存在或已退出
/// 则静默 ESRCH）；windows `TerminateJobObject`，无 job 时退化为只杀
/// 主进程。
pub async fn kill_tree(child: &mut tokio::process::Child, tree: &ProcessTree) {
    #[cfg(unix)]
    {
        if tree.pgid > 0 {
            // SAFETY: 对进程组发信号，pid 无效时返回 ESRCH，无副作用。
            unsafe { kill(-tree.pgid, SIGKILL) };
        }
        let _ = child.wait().await; // 收割僵尸
    }
    #[cfg(windows)]
    {
        if let Some(job) = &tree.job {
            let _ = job.terminate(1);
        }
        // terminate 与「主进程已自行退出」竞态并存：kill 已退出进程返
        // 回 Err，忽略；同时完成收割。
        let _ = child.kill().await;
    }
}

/// 按 pid 终止整棵树（只知 pid 的管理面用，如后台任务 tracker）：
/// unix 假定 pid 是 [`spawn_in_new_tree`] 拉起的进程组组长，向全组发
/// SIGTERM；windows 用 `taskkill /T /F`（该工具按树强杀，无温和档）。
/// 平台温和度不同，调用方语义统一为「请整棵树尽快退场」。
pub fn terminate_tree_by_pid(pid: u32) -> io::Result<()> {
    // unix 上 kill(0, sig) 打的是「调用方自身进程组」——tracker 在
    // child.id() 缺失时会存入 0，这里兜底防自残。
    if pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pid 0 has no process tree",
        ));
    }
    #[cfg(unix)]
    {
        // SAFETY: 对进程组发信号，组不存在返回 ESRCH，由调用方按失败处理。
        if unsafe { kill(-(pid as i32), SIGTERM) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let status = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("taskkill exited with {status}")))
        }
    }
}

#[cfg(all(test, unix))]
#[path = "process_test.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "process_windows_test.rs"]
mod windows_tests;
