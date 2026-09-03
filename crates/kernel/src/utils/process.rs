//! 子进程 spawn 工具。

// POSIX `setsid(2)`：与 libc 同 linker 命名空间，unix 上始终已链接，
// 手动声明以避免 libc/nix 依赖。
#[cfg(unix)]
extern "C" {
    fn setsid() -> i32;
}

/// 让子进程独立成新 session（`setsid`）：子进程成为新进程组的组长
/// （pgid == 子 pid），超时/收尾时按组发信号能连后裔一起收——只杀
/// 直接子进程会让 `sleep 60 &` 型后裔继续持有管道/资源。unix 之外
/// 为 no-op。
///
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

#[cfg(test)]
#[path = "process_test.rs"]
mod tests;
