//! Daemon 单例锁：同一个 `data_dir` 同时只允许一个启用 cron 的 kernel。
//!
//! 背景：pid file 由 socket 地址派生（[`crate::transport::pid_file_path`]），
//! 所以两个 socket 不同（如测试 daemon 自带 `YOMI_SOCKET`）但 `data_dir`
//! 相同（继承了注入的 `YOMI_DATA_DIR=~/.yomi`）的 daemon 会各自认为
//! 自己合法、同时跑两份 cron 调度器（2026-08-19 双发事故）。本锁以
//! *解析后的 `data_dir`* 为键：环境隔离（不同 `YOMI_DATA_DIR`）的实例
//! 天然并行；同 `data_dir` 第二个实例在启动时直接失败。
//!
//! Unix：对 `/tmp` 下的 `yomi-daemon-<uid>-<hash>.lock` 持有
//! `flock(2)`，生命周期覆盖 kernel 存活期，由 `Kernel::stop()` 末尾
//! 提前释放（锁的生命期 = 调度器生命期，restart 交接因此是确定性
//! 的）；进程崩溃由内核自动释放，不存在 stale 锁。锁放 tmp 而不进
//! `data_dir`：`data_dir` 是持久数据目录，锁是运行时文件——升级/替换
//! `data_dir` 内容不该碰锁；tmp 由系统定期清理（Linux 重启即清，
//! macOS 按文件龄期），文件残骸无害——flock 随进程死亡释放，重启后
//! 抢到锁会覆盖重写。
//!
//! 目录固定 `/tmp` 而非 `std::env::temp_dir()`：后者吃 `$TMPDIR`，
//! 同一 `data_dir` 的 daemon 与竞争者可能来自 TMPDIR 不同的启动环境
//! （launchd / ssh / cron），键会因此分裂、互斥失效。/tmp 全机唯一，
//! 文件名里的 uid 防跨用户撞键（非 unix 用 per-user 的 `temp_dir()`）。
//!
//! 升级窗口：旧版 yomi 在 `<data_dir>/daemon.lock` 上持 flock 且只认
//! 它。新版锁在 tmp，升级交接期新旧各锁各的、互斥失效（双发 cron）。
//! 因此 legacy 锁文件存在时同样非阻塞抢一把：抢不到 = 有旧版 daemon
//! 在跑，报 `Contended`；抢到则随守卫一直持有，旧版竞争者只看 legacy
//! 锁，这样它们也能被正确拒绝。文件不存在则不创建——`data_dir` 不再
//! 新增运行时文件。
//!
//! `<hash>` 是规范化 `data_dir` 路径的 FNV-1a（见 [`lock_stem`]）。
//! `flock` 无法报告持有者身份，获取成功后顺手写一份 best-effort 的
//! meta（pid/exe/socket/启动时间，锁文件同名加 `.meta`），被拒绝的
//! 竞争方据此拼诊断信息。
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
    /// 旧版锁文件（`<data_dir>/daemon.lock`）的 flock：仅当该文件已
    /// 存在（有旧版 daemon 的痕迹）时持有，用于升级窗口互斥。
    #[cfg(unix)]
    _legacy_lock: Option<nix::fcntl::Flock<std::fs::File>>,
    path: PathBuf,
}

impl std::fmt::Debug for DataDirGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("DataDirGuard");
        debug.field("path", &self.path);
        #[cfg(unix)]
        {
            debug.field("locked", &self._lock.is_some());
            debug.field("legacy_locked", &self._legacy_lock.is_some());
        }
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

/// 锁目录：unix 固定 `/tmp`（原因见模块文档；`$TMPDIR` 会把键劈裂）。
/// 非 unix 用 per-user 的 `std::env::temp_dir()`。
fn lock_dir() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/tmp")
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir()
    }
}

pub fn lock_file_path(data_dir: &Path) -> PathBuf {
    lock_dir().join(format!("{}.lock", lock_stem(data_dir)))
}

pub fn meta_file_path(data_dir: &Path) -> PathBuf {
    lock_dir().join(format!("{}.lock.meta", lock_stem(data_dir)))
}

/// 锁文件名的稳定键：`yomi-daemon-<uid>-<hash>`。
///
/// - `hash`：规范化后 `data_dir` 路径的 FNV-1a。symlink/相对路径指向
///   同一物理目录的两个 daemon 必须有同一个键，否则各拿各的锁、
///   双发依旧。
/// - `uid`（仅 unix）：/tmp 全机共享，不同用户的同名 `data_dir`
///   各拿各的键。
fn lock_stem(data_dir: &Path) -> String {
    // 非 UTF-8 路径按 lossy 形式哈希：两个不同非 UTF-8 目录理论上可能
    // 撞成同一个键（表现为无谓互斥，不会放跑双发），可接受。
    let hash = fnv1a_64(canonical_key(data_dir).to_string_lossy().as_bytes());
    #[cfg(unix)]
    let uid = nix::unistd::getuid().to_string();
    #[cfg(not(unix))]
    let uid = String::new();
    format!("yomi-daemon-{uid}-{hash:016x}")
}

/// 规范化路径作哈希键。目录还不存在时（如 `daemon lock-path` 对未初
/// 始化的 `data_dir` 查询）逐层向上找存在的祖先、拼回剩余部分——得到
/// 与"创建后 canonicalize"一致的结果，否则查询方与运行方会指向两个
/// 不同的锁。先把绝对化路径做一遍词法规整（弹掉 `..`，`file_name()`
/// 对 `..` 段返回 None 会被静默丢弃）；整条链都规范化不了（极端环境）
/// 退回词法规整后的原路径。
fn canonical_key(data_dir: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(data_dir) {
        return p;
    }
    let abs = normalize_lexical(
        &std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf()),
    );
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut cur: &Path = &abs;
    loop {
        if let Ok(base) = std::fs::canonicalize(cur) {
            let mut acc = base;
            for c in missing.iter().rev() {
                acc.push(c);
            }
            return acc;
        }
        match cur.parent() {
            Some(parent) => {
                if let Some(name) = cur.file_name() {
                    missing.push(name.to_os_string());
                }
                cur = parent;
            }
            None => return abs,
        }
    }
}

/// 词法规整：弹掉 `..`（吃掉前一个普通分量）、丢掉 `.`。只用于
/// canonicalize 失败的前置处理，不访问文件系统。
fn normalize_lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// FNV-1a 64：稳定、零依赖的哈希。std 的 `DefaultHasher` 明确不保证
/// 跨版本稳定，不能拿来做持久键，所以手写一个。
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 尝试获取 `data_dir` 单例锁（非阻塞）。调用方须保证 `data_dir` 已存在。
pub fn acquire(data_dir: &Path) -> Result<DataDirGuard, AcquireError> {
    imp::acquire(
        lock_file_path(data_dir),
        &meta_file_path(data_dir),
        data_dir,
    )
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
    use std::path::PathBuf;

    pub(super) fn acquire(
        path: PathBuf,
        meta_path: &std::path::Path,
        data_dir: &std::path::Path,
    ) -> Result<DataDirGuard, AcquireError> {
        let lock = try_flock(&path)?;
        match lock {
            Ok(lock) => {
                // 升级窗口互斥：legacy 锁文件（旧版 yomi 的痕迹）在就抢，
                // 详见模块文档。tmp 锁已到手，此处失败会走 `?` 让
                // guard 构造失败、tmp 锁随 `lock` 的 drop 释放，不留半截。
                // 注意顺序：先 legacy 再写 meta——否则 legacy 撞车时会把
                // 本进程（即将退出）的 pid 写进 meta，下一个竞争方会读到
                // 一个死进程的持有者信息。
                let legacy_lock = acquire_legacy(
                    &data_dir.join("daemon.lock"),
                    &data_dir.join("daemon.lock.meta"),
                )?;
                write_meta(&lock, &path, meta_path);
                Ok(DataDirGuard {
                    _lock: Some(lock),
                    _legacy_lock: legacy_lock,
                    path,
                })
            }
            Err((_file, nix::errno::Errno::EWOULDBLOCK)) => Err(AcquireError::Contended {
                owner: read_owner_meta(meta_path),
            }),
            Err((_file, e)) => Err(AcquireError::Io(std::io::Error::from(e))),
        }
    }

    /// 非阻塞 exclusive flock，EINTR 重试（nix 的 `Flock::lock` 单次
    /// syscall 不重试；信号恰好落在非阻塞 flock 上会报 "Interrupted
    /// system call" 让启动假失败）。
    fn flock_nb(
        mut file: std::fs::File,
    ) -> Result<Flock<std::fs::File>, (std::fs::File, nix::errno::Errno)> {
        loop {
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Err((f, nix::errno::Errno::EINTR)) => file = f,
                result => return result,
            }
        }
    }

    fn try_flock(
        path: &std::path::Path,
    ) -> Result<Result<Flock<std::fs::File>, (std::fs::File, nix::errno::Errno)>, AcquireError>
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .create(true)
            // 不截断：锁的是文件描述符，内容（若有）无关紧要。
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            // /tmp 全机可写，锁文件名可预测（data_dir 路径哈希）：预置
            // symlink 会让 create(true) 跟随链接去打开攻击者选的文件。
            // O_NOFOLLOW 让这种预置以 ELOOP 硬失败收场（sticky bit 下
            // 我们也删不掉别人的文件，不能自愈，只能报错）。
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(path)
            .map_err(AcquireError::Io)?;
        Ok(flock_nb(file))
    }

    /// 旧版（tmp 锁之前）yomi 在 `<data_dir>/daemon.lock` 上持 flock 且
    /// 只认它。文件存在就非阻塞抢：抢不到 = 旧版 daemon 在跑，报
    /// Contended（owner 从 legacy meta 读）；抢到随守卫持有，让旧版
    /// 竞争者继续被拒。文件不存在返回 None——不创建，`data_dir` 不新增
    /// 运行时文件。
    fn acquire_legacy(
        legacy_path: &std::path::Path,
        legacy_meta_path: &std::path::Path,
    ) -> Result<Option<Flock<std::fs::File>>, AcquireError> {
        if !legacy_path.exists() {
            return Ok(None);
        }
        // LOCK_EX 需要写权限打开的 fd。
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(legacy_path)
        {
            Ok(f) => f,
            // 与旧版进程的清理竞态（它退出时删文件）：当作没有 legacy 锁。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AcquireError::Io(e)),
        };
        match flock_nb(file) {
            Ok(legacy) => Ok(Some(legacy)),
            Err((_file, nix::errno::Errno::EWOULDBLOCK)) => Err(AcquireError::Contended {
                owner: read_owner_meta(legacy_meta_path),
            }),
            Err((_file, e)) => Err(AcquireError::Io(std::io::Error::from(e))),
        }
    }

    /// 锁已到手；顺手把身份写进 meta，供未来的竞争方诊断。
    /// 任何一步失败都只记日志——不影响持锁本身。
    fn write_meta(
        _lock: &Flock<std::fs::File>,
        lock_path: &std::path::Path,
        meta_path: &std::path::Path,
    ) {
        use std::os::unix::fs::OpenOptionsExt;
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
            // 与锁文件同款防护：meta 文件名同样可预测，且是截断写——
            // 被预置 symlink 命中会清掉攻击者选的文件。0o600：meta 含
            // pid/socket，不给别人看。
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW)
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
            Err(e) => tracing::warn!(
                meta = %meta_path.display(),
                "lock acquired but failed to write owner metadata: {e}"
            ),
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::{AcquireError, DataDirGuard};
    use std::path::PathBuf;

    pub(super) fn acquire(
        path: PathBuf,
        _meta_path: &std::path::Path,
        _data_dir: &std::path::Path,
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
