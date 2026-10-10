//! Daemon 单例锁：同一个 `data_dir` 同时只允许一个启用 cron 的 kernel。
//!
//! 背景：pid file 由 socket 地址派生（[`crate::transport::pid_file_path`]），
//! 所以两个 socket 不同（如测试 daemon 自带 `YOMI_SOCKET`）但 `data_dir`
//! 相同（继承了注入的 `YOMI_DATA_DIR=~/.yomi`）的 daemon 会各自认为
//! 自己合法、同时跑两份 cron 调度器（2026-08-19 双发事故）。本锁以
//! *解析后的 `data_dir`* 为键：环境隔离（不同 `YOMI_DATA_DIR`）的实例
//! 天然并行；同 `data_dir` 第二个实例在启动时直接失败。
//!
//! 锁住在 `<data_dir>/run/daemon.lock`，meta 同名加 `.meta`。对该文件
//! 持 `flock(2)`，生命周期覆盖 kernel 存活期，由 `Kernel::stop()` 末尾
//! 提前释放（锁的生命期 = 调度器生命期，restart 交接因此是确定性的）；
//! 进程崩溃由内核自动释放，不存在 stale 锁。
//!
//! 锁放私有目录而非 /tmp：无 symlink 预置、跨用户撞键、`$TMPDIR` 劈键
//! 问题，路径即键，无需哈希。`run/` 子目录把运行时文件归拢，备份/同步
//! 排除一个目录即可。
//!
//! `flock` 无法报告持有者身份，获取成功后顺手写一份 best-effort 的
//! meta（pid/exe/socket/启动时间），被拒绝的竞争方据此拼诊断信息。
//!
//! Windows：暂未实现（no-op 守卫，直接放行）。

use std::path::{Path, PathBuf};

/// 锁持有者信息，获取成功后 best-effort 写入 meta 文件。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LockOwner {
    pub pid: u32,
    pub exe: Option<String>,
    pub socket: Option<String>,
    pub started_at: String,
}

impl LockOwner {
    /// 一行人类可读描述，用于错误信息。
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("pid {}", self.pid)];
        if let Some(exe) = &self.exe {
            parts.push(exe.clone());
        }
        if let Some(socket) = &self.socket {
            parts.push(format!("socket {socket}"));
        }
        parts.push(format!("started {}", self.started_at));
        parts.join(", ")
    }
}

/// 获取锁失败的原因。
#[derive(Debug)]
pub enum AcquireError {
    /// 另一个存活进程持有该 `data_dir` 的锁。
    Contended {
        owner: Option<LockOwner>,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Contended { owner: Some(owner) } => {
                write!(f, "already owned by {}", owner.describe())
            }
            Self::Contended { owner: None } => {
                write!(
                    f,
                    "already owned by another process (owner metadata unreadable)"
                )
            }
            Self::Io(e) => write!(f, "lock file I/O error: {e}"),
        }
    }
}

/// 守卫：持有期间锁不释放。Drop 即释放；`Kernel::stop()` 会提前 take
/// 并 drop（restart 场景在新 kernel 构建前释放）。
pub struct DataDirGuard {
    /// flock 守卫（仅 unix 平台实际持锁；Windows 无此字段）。
    #[cfg(unix)]
    _lock: Option<nix::fcntl::Flock<std::fs::File>>,
    path: PathBuf,
}

impl std::fmt::Debug for DataDirGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("DataDirGuard");
        debug.field("path", &self.path);
        #[cfg(unix)]
        debug.field("locked", &self._lock.is_some());
        debug.finish()
    }
}

impl DataDirGuard {
    pub fn lock_path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DataDirGuard {
    fn drop(&mut self) {
        tracing::info!(lock = %self.path.display(), "daemon singleton lock released");
    }
}

/// 锁目录：`<canonical data_dir>/run`。调用方有的传规范化路径（
/// `build_kernel`）、有的传配置原文（GUI 回退、`daemon lock-path`）——
/// 在这里统一规范化，两边才指向同一把锁。规范化失败（目录不存在）
/// 退回原路径，调用方负责保证存在。
fn run_dir(data_dir: &Path) -> PathBuf {
    let base = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    base.join("run")
}

pub fn lock_file_path(data_dir: &Path) -> PathBuf {
    run_dir(data_dir).join("daemon.lock")
}

pub fn meta_file_path(data_dir: &Path) -> PathBuf {
    run_dir(data_dir).join("daemon.lock.meta")
}

/// 尝试获取 `data_dir` 单例锁（非阻塞）。`run/` 子目录（连同不存在的
/// `data_dir` 本身）不存在时自动创建。
pub fn acquire(data_dir: &Path) -> Result<DataDirGuard, AcquireError> {
    imp::acquire(lock_file_path(data_dir), &meta_file_path(data_dir))
}

fn read_owner_meta(meta_path: &Path) -> Option<LockOwner> {
    let content = std::fs::read_to_string(meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 读取上次成功持锁者留下的 meta（best-effort，可能不存在或已过期）。
pub fn read_owner(data_dir: &Path) -> Option<LockOwner> {
    read_owner_meta(&meta_file_path(data_dir))
}

#[cfg(unix)]
mod imp {
    use super::{read_owner_meta, AcquireError, DataDirGuard, LockOwner};
    use nix::fcntl::{Flock, FlockArg};
    use std::os::unix::fs::OpenOptionsExt;

    pub(super) fn acquire(
        path: std::path::PathBuf,
        meta_path: &std::path::Path,
    ) -> Result<DataDirGuard, AcquireError> {
        // run/ 不存在（如全新 data_dir）时补上；创建失败按 I/O 错误上抛。
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(AcquireError::Io)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            // 不截断：锁的是文件描述符，内容（若有）无关紧要。
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(AcquireError::Io)?;
        // nix 的 Flock::lock 是单次 syscall 不重试 EINTR；信号恰好落在
        // 非阻塞 flock 上会报 "Interrupted system call" 让启动假失败。
        let lock = loop {
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Err((f, nix::errno::Errno::EINTR)) => file = f,
                result => break result,
            }
        };
        match lock {
            Ok(lock) => {
                write_meta(&path, meta_path);
                Ok(DataDirGuard {
                    _lock: Some(lock),
                    path,
                })
            }
            Err((_file, nix::errno::Errno::EWOULDBLOCK)) => Err(AcquireError::Contended {
                owner: read_owner_meta(meta_path),
            }),
            Err((_file, e)) => Err(AcquireError::Io(std::io::Error::from(e))),
        }
    }

    /// 锁已到手；顺手把身份写进 meta，供未来的竞争方诊断。
    /// 任何一步失败都只记日志——不影响持锁本身。
    fn write_meta(lock_path: &std::path::Path, meta_path: &std::path::Path) {
        let owner = LockOwner {
            pid: std::process::id(),
            exe: std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string()),
            socket: crate::transport::try_socket_addr().map(|a| a.to_string()),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        match std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            // meta 含 pid/socket，不给别人看。
            .mode(0o600)
            .open(meta_path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(serde_json::to_string_pretty(&owner)?.as_bytes())?;
                // sync meta 文件本身（不是锁 fd——那对 meta 的持久化毫无意义）。
                f.sync_all()
            }) {
            Ok(()) => tracing::info!(
                lock = %lock_path.display(),
                pid = owner.pid,
                "daemon singleton lock acquired"
            ),
            Err(e) => {
                // 写失败时清掉旧 meta：文件里可能还是上一任（已死）
                // 持有者的 pid，被回收后竞争方/停机方会误读持有者身份、
                // 把信号发给无辜进程。宁可没有 meta（owner=None 走安全
                // 拒绝路径），不留可能撒谎的旧身份。
                let _ = std::fs::remove_file(meta_path);
                tracing::warn!(
                    meta = %meta_path.display(),
                    "lock acquired but failed to write owner metadata: {e}"
                );
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::{AcquireError, DataDirGuard};

    pub(super) fn acquire(
        path: std::path::PathBuf,
        _meta_path: &std::path::Path,
    ) -> Result<DataDirGuard, AcquireError> {
        tracing::warn!(
            lock = %path.display(),
            "daemon singleton lock is not implemented on this platform; \
             concurrent daemons on one data dir are NOT prevented"
        );
        Ok(DataDirGuard { path })
    }
}

#[cfg(all(test, unix))]
#[path = "daemon_lock_test.rs"]
mod tests;
