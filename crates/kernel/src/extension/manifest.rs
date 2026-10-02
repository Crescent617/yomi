//! ext.toml manifest 解析与校验。
//!
//! 校验在 install 时硬拒绝（宁可装不上，不装半个）：名字规则、
//! `message`/`message_file` 二选一、manifest 内相对路径不得越出包根
//!（`../../etc/x` 式的 `message_file` 会让 install 读任意文件）、
//! `schedule` 必须有未来触发点（复用 cron 子系统的校验）。

use std::io::Read as _;
use std::path::Path;

use serde::Deserialize;

use super::{ExtError, MANIFEST_FILE};

/// ext.toml 顶层结构。
#[derive(Debug, Clone, Deserialize)]
pub struct ExtManifest {
    pub ext: ExtMeta,
    /// cron 收养条目（全名 `ext:<扩展名>:<条目名>`）。
    #[serde(default)]
    pub cron: Vec<CronEntry>,
}

/// `[ext]` 段：扩展自身的身份。
#[derive(Debug, Clone, Deserialize)]
pub struct ExtMeta {
    /// 扩展名：字母开头，`[a-z0-9-]`，≤32。同时是 extensions/ 槽位名、
    /// cron 命名空间、snippet 展示名——一个名字贯穿全部资源。
    pub name: String,
    /// 展示/审计用，不参与兼容判断（v1 无版本求解）。
    pub version: String,
    /// `extension list` 展示用。
    pub description: String,
    /// 可选：安装钩子——装完/刷新后从**已装目录**执行的包内脚本
    /// （相对包根路径）。执行环境是标准 yomi 子进程环境（注入
    /// `YOMI_DATA_DIR`，PATH 含 `<data_dir>/bin`，cwd = 包目录）。
    /// 每次 install/refresh 都跑（ensure 哲学，幂等是作者约定）；
    /// 声明了就必须存在、不出包根、不含空白字符（shell 命令文本
    /// 注入，空白会断词）。
    pub init: Option<String>,
}

/// `[[cron]]` 条目：收养为 ensure-by-name 的 `send_message` job。
#[derive(Debug, Clone, Deserialize)]
pub struct CronEntry {
    /// 条目名（≤48）：cron 全名的末段。
    pub name: String,
    /// 5/6 段 cron 表达式，本地时区（cron 子系统既有语义）。
    pub schedule: String,
    /// 内联消息文本。与 `message_file` 二选一。
    pub message: Option<String>,
    /// 包内文件（相对包根）作消息文本。与 `message` 二选一。
    pub message_file: Option<String>,
    /// 可选：per-run 会话工作目录（经 `session_template` 透传，cron 归一化语义）。
    pub work_dir: Option<String>,
    /// 可选：传感器闸门命令（cron 子系统既有语义）。
    #[allow(clippy::doc_markdown)]
    pub precheck: Option<String>,
}

/// 解析并校验包根目录下的 ext.toml。
pub fn parse_manifest(pkg_dir: &Path) -> Result<ExtManifest, ExtError> {
    let path = pkg_dir.join(MANIFEST_FILE);
    // 只读常规文件：FIFO/设备文件会让 daemon 的 RPC 路径无限阻塞。
    require_regular_file(&path)?;
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| ExtError::Invalid(format!("read {}: {e}", path.display())))?;
    let manifest: ExtManifest =
        toml::from_str(&raw).map_err(|e| ExtError::Invalid(format!("parse ext.toml: {e}")))?;
    manifest.validate(pkg_dir)?;
    Ok(manifest)
}

/// 路径必须是常规文件（跟随 symlink 后的目标也算）。FIFO/设备文件的
/// open 会无限阻塞——install 跑在 daemon 的 RPC 路径上，一个恶意包能
/// 挂死整个 daemon。
fn require_regular_file(path: &Path) -> Result<(), ExtError> {
    let md = std::fs::metadata(path)
        .map_err(|e| ExtError::Invalid(format!("stat {}: {e}", path.display())))?;
    if !md.is_file() {
        return Err(ExtError::Invalid(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    Ok(())
}

/// `message` 文本上限（对齐 channel rules 的量级：约定是"要点"不是文章）。
pub const MESSAGE_MAX_BYTES: usize = 64 * 1024;
/// `ext.description` 上限（list 展示用）。
pub const DESCRIPTION_MAX_CHARS: usize = 256;
/// `ext.version` 上限（展示/审计用，不参与兼容求解）。
pub const VERSION_MAX_CHARS: usize = 64;
/// `[[cron]]` 条目数上限：remove 的前缀清扫一次取有限批，条目无界会让
/// 清扫漏尾（remove 不完整且无告警）。256 条远超真实扩展的用量。
pub const CRON_ENTRIES_MAX: usize = 256;
/// `ext.init` 脚本大小上限（约定是初始化钩子，不是程序载体）。
pub const INIT_MAX_BYTES: usize = 1024 * 1024;

impl ExtManifest {
    fn validate(&self, pkg_dir: &Path) -> Result<(), ExtError> {
        if !valid_ext_name(&self.ext.name) {
            return Err(ExtError::Invalid(format!(
                "ext.name '{}' invalid: letter first, [a-z0-9-] only, ≤32 chars",
                self.ext.name
            )));
        }
        if self.ext.version.trim().is_empty() {
            return Err(ExtError::Invalid(
                "ext.version must not be empty".to_string(),
            ));
        }
        if self.ext.version.chars().count() > VERSION_MAX_CHARS {
            return Err(ExtError::Invalid(format!(
                "ext.version too long (>{VERSION_MAX_CHARS} chars)"
            )));
        }
        if self.ext.description.trim().is_empty() {
            return Err(ExtError::Invalid(
                "ext.description must not be empty".to_string(),
            ));
        }
        if self.ext.description.chars().count() > DESCRIPTION_MAX_CHARS {
            return Err(ExtError::Invalid(format!(
                "ext.description too long (>{DESCRIPTION_MAX_CHARS} chars)"
            )));
        }
        if self.cron.len() > CRON_ENTRIES_MAX {
            return Err(ExtError::Invalid(format!(
                "too many cron entries ({} > {CRON_ENTRIES_MAX})",
                self.cron.len()
            )));
        }
        if let Some(rel) = &self.ext.init {
            // 绝对路径硬拒：join 会被整体替换，装到目标位置后必 127。
            if std::path::Path::new(rel).is_absolute() {
                return Err(ExtError::Invalid(format!(
                    "ext.init '{rel}' must be a package-relative path"
                )));
            }
            // 越界拒绝：join 后必须仍在包根之下（同 message_file 口径；
            // 脚本以 daemon 身份执行，能读包外 = 能读一切）。
            let abs = pkg_dir.join(rel);
            let canonical = abs
                .canonicalize()
                .map_err(|e| ExtError::Invalid(format!("ext.init '{rel}': {e}")))?;
            let root = pkg_dir
                .canonicalize()
                .map_err(|e| ExtError::Invalid(format!("canonicalize package dir: {e}")))?;
            if !canonical.starts_with(&root) {
                return Err(ExtError::Invalid(format!(
                    "ext.init '{rel}' escapes the package"
                )));
            }
            // 常规文件 + 上限：FIFO 会挂死 daemon 的 RPC 路径（install
            // 同步等脚本退出）。空白字符硬拒：init 经 shell 命令文本
            // 执行（复用 wrap_command），断词会跑成不可预期的命令。
            require_regular_file(&canonical)
                .map_err(|e| ExtError::Invalid(format!("ext.init '{rel}': {e}")))?;
            if rel
                .chars()
                .any(|c| c.is_whitespace() || c == '"' || c == '\'')
            {
                return Err(ExtError::Invalid(format!(
                    "ext.init '{rel}': whitespace and quotes not allowed in script path"
                )));
            }
            let size = std::fs::metadata(&canonical)
                .map_err(|e| ExtError::Invalid(format!("stat ext.init: {e}")))?
                .len();
            if size > INIT_MAX_BYTES as u64 {
                return Err(ExtError::Invalid(format!(
                    "ext.init '{rel}' too large (>{INIT_MAX_BYTES} bytes)"
                )));
            }
        }
        for entry in &self.cron {
            entry.validate(pkg_dir)?;
        }
        // 同名条目重复：第二条会静默报 exists（ensure）而其 schedule/
        // message 被丢弃——装半个 definition 不如拒绝。
        let mut names = std::collections::BTreeSet::new();
        for entry in &self.cron {
            if !names.insert(&entry.name) {
                return Err(ExtError::Invalid(format!(
                    "duplicate cron entry name '{}'",
                    entry.name
                )));
            }
        }
        Ok(())
    }
}

impl CronEntry {
    fn validate(&self, pkg_dir: &Path) -> Result<(), ExtError> {
        if !valid_entry_name(&self.name) {
            return Err(ExtError::Invalid(format!(
                "cron entry name '{}' invalid: letter first, [a-zA-Z0-9_-] only, ≤48 chars",
                self.name
            )));
        }
        match (&self.message, &self.message_file) {
            (Some(_), Some(_)) => {
                return Err(ExtError::Invalid(format!(
                    "cron entry '{}' sets both message and message_file (exactly one required)",
                    self.name
                )));
            }
            (None, None) => {
                return Err(ExtError::Invalid(format!(
                    "cron entry '{}' sets neither message nor message_file (exactly one required)",
                    self.name
                )));
            }
            _ => {}
        }
        if let Some(rel) = &self.message_file {
            // 越界拒绝：join 后必须仍在包根之下（防 ../../ 读任意文件）。
            let abs = pkg_dir.join(rel);
            let canonical = abs.canonicalize().map_err(|e| {
                ExtError::Invalid(format!(
                    "cron entry '{}': message_file '{rel}': {e}",
                    self.name
                ))
            })?;
            let root = pkg_dir
                .canonicalize()
                .map_err(|e| ExtError::Invalid(format!("canonicalize package dir: {e}")))?;
            if !canonical.starts_with(&root) {
                return Err(ExtError::Invalid(format!(
                    "cron entry '{}': message_file '{rel}' escapes the package",
                    self.name
                )));
            }
            // 常规文件 + 大小上限：FIFO 会挂死 daemon 的 RPC 路径；无界
            // 文件会全量进内存与 cron 表。
            require_regular_file(&canonical).map_err(|e| {
                ExtError::Invalid(format!(
                    "cron entry '{}': message_file '{rel}': {e}",
                    self.name
                ))
            })?;
            let size = std::fs::metadata(&canonical)
                .map_err(|e| ExtError::Invalid(format!("stat message_file: {e}")))?
                .len();
            if size > MESSAGE_MAX_BYTES as u64 {
                return Err(ExtError::Invalid(format!(
                    "cron entry '{}': message_file '{rel}' too large ({size} > {MESSAGE_MAX_BYTES} bytes)",
                    self.name
                )));
            }
        }
        if let Some(text) = &self.message {
            if text.len() > MESSAGE_MAX_BYTES {
                return Err(ExtError::Invalid(format!(
                    "cron entry '{}': message too long ({} > {MESSAGE_MAX_BYTES} bytes)",
                    self.name,
                    text.len()
                )));
            }
        }
        // schedule 必须有未来触发点（与 cron 子系统 create 路径同一校验，
        // 在 install 时早失败，而不是收养到一半才报错）。
        crate::cron::next_run_from_schedule(&self.schedule)
            .map_err(|e| ExtError::Invalid(format!("cron entry '{}': schedule: {e}", self.name)))?;
        Ok(())
    }

    /// 解析消息文本（内联或包内文件），install/展示用。
    pub fn resolve_message(&self, pkg_dir: &Path) -> Result<String, ExtError> {
        match (&self.message, &self.message_file) {
            (Some(text), _) => Ok(text.clone()),
            (None, Some(rel)) => {
                let path = pkg_dir.join(rel);
                require_regular_file(&path)?;
                // 限长读：validate 检过大小，但校验与读取之间文件可被
                // 换掉——对齐 snippet 的 read_bounded，无界 read 不进
                // 内存。
                let mut file = std::fs::File::open(&path)?;
                let mut buf = Vec::new();
                file.by_ref()
                    .take(MESSAGE_MAX_BYTES as u64 + 1)
                    .read_to_end(&mut buf)
                    .map_err(|e| ExtError::Invalid(format!("read {}: {e}", path.display())))?;
                if buf.len() > MESSAGE_MAX_BYTES {
                    return Err(ExtError::Invalid(format!(
                        "cron entry '{}': message_file grew beyond {MESSAGE_MAX_BYTES} bytes",
                        self.name
                    )));
                }
                String::from_utf8(buf).map_err(|e| {
                    ExtError::Invalid(format!("message_file {} not UTF-8: {e}", path.display()))
                })
            }
            (None, None) => Err(ExtError::Invalid(format!(
                "cron entry '{}': no message",
                self.name
            ))),
        }
    }
}

/// 扩展名：字母开头，`[a-z0-9-]`（小写——它要进路径与 cron 名），≤32。
pub fn valid_ext_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// 条目名：字母开头，`[a-zA-Z0-9_-]`（cron 名无小写限制），≤48。
pub fn valid_entry_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
#[path = "manifest_test.rs"]
mod tests;
