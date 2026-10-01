//! extension —— 扩展包（extension packages）：安装与生命周期层。
//!
//! 定位见 `docs/design/ext-packages.md`：不是新的扩展端口，而是把已有
//! 注册表（hooks/bin 目录、cron 表、SP snippet 拼装）的**物化与回收**
//! 自动化。运行时语义全部不变——hook 扫描、执行位开关、cron ensure、
//! snippet 快照拼装照旧；包只改变资源"怎么进来、怎么出去"。
//!
//! 核心规则（设计的承重墙）：
//! - **所有权 = symlink 文本目标**：install 只会创建目标为
//!   `extensions/<名>/...` 的 symlink，或刷新已指向本包的；绝不覆盖
//!   用户文件或其他扩展的挂载。任何中断的安装可重跑收敛到同一终态。
//! - **cron 全名 `ext:<扩展名>:<条目名>`**：命名空间即所有权标记，
//!   remove 按前缀清扫。
//! - **install 纯 additive**：cron 缺才建、已存在不动（防覆盖用户手改）；
//!   清扫只在 remove 发生。
//! - **正确性不依赖 sqlite 安装记录**（state is cache）：记录只做审计
//!   与 list 展示；remove 记录缺失时退化到"扫包目录 + cron 前缀"。

mod install;
mod installed;
mod manifest;
mod snippets;
mod source;

pub use install::{
    install, package_hash, remove, InstallReport, MountReport, MountStatus, RemoveReport,
};
pub use installed::{
    list_installed, read_installed, write_install_meta, InstallMeta, InstalledExt, Provenance,
    Resources,
};
pub use manifest::{parse_manifest, CronEntry, ExtManifest, ExtMeta};
pub use snippets::{load_snippets, Snippet, SnippetLoader};
pub use source::{fetch_source, parse_source, ExtSource};

/// 包内 manifest 文件名（作者手写，install 后原封不动）。
pub const MANIFEST_FILE: &str = "ext.toml";
/// 安装锁文件名：install 盖在已装目录里的溯源/资源清单（等价
/// Cargo.lock 对 Cargo.toml——manifest 归作者，lock 归工具）。
/// 存在与否 = 目录是不是 yomi 装的（foreign 判定）；hash 时整体跳过。
pub const LOCK_FILE: &str = "ext.lock";
/// 已安装扩展的库目录名（相对 `data_dir`）。
pub const DIR_NAME: &str = "extensions";
/// 包内 snippet 目录名（约定式资源，文件名排序拼 SP）。
pub const SNIPPETS_DIR: &str = "snippets";
/// 包内 hook 条目目录名：`hooks/<point>/<entry>`。
pub const HOOKS_DIR: &str = "hooks";
/// 包内 bin 目录名：可执行文件挂进 `<data_dir>/bin`（PATH 层）。
pub const BIN_DIR: &str = "bin";

/// cron job 全名前缀：`ext:<扩展名>:`（所有权标记，remove 按前缀清扫）。
pub fn cron_name(ext: &str, entry: &str) -> String {
    format!("ext:{ext}:{entry}")
}

/// 扩展包错误。
#[derive(Debug, thiserror::Error)]
pub enum ExtError {
    /// manifest 非法（字段缺失/名字非法/message 二选一违规/路径越界/schedule 永不触发）。
    #[error("invalid package: {0}")]
    Invalid(String),
    /// 取货失败（clone 超时/非零退出/spawn 失败/tempdir）。与 manifest
    /// 非法区分：用户看到 fetch failed 去查网络与源仓，而不是改 ext.toml。
    #[error("fetch failed: {0}")]
    Fetch(String),
    /// I/O 失败（读包、建/摘 symlink、目录操作）。
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// cron 收养失败。
    #[error("cron: {0}")]
    Cron(#[from] crate::cron::CronError),
    /// 挂载槽位被占（用户文件或其他扩展）。报全文，不静默跳过。
    #[error("mount conflict: {0}")]
    Conflict(String),
}

impl From<ExtError> for crate::types::KernelError {
    fn from(e: ExtError) -> Self {
        match e {
            ExtError::Io(io) => crate::types::KernelError::Io(io.to_string()),
            ExtError::Cron(c) => c.into(),
            // 消息自身已可行动（"invalid package: …"/"fetch failed: …"/
            // "mount conflict: …"），透明透传，不加误导前缀。
            other => crate::types::KernelError::Extension(other.to_string()),
        }
    }
}
