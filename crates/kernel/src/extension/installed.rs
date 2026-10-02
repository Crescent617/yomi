//! 已装扩展的注册表：**单个** `extensions/ext.lock`（等价 Cargo.lock
//! 对 Cargo.toml——ext.toml 归作者，lock 归工具），`[[extensions]]`
//! 按名字一字一条。目录 + 一个 lock 即全部状态，没有 sqlite 表。
//!
//! 为什么 lock 是包外单文件：
//! - 包内容完全来自作者——lock 放包目录里，作者随包自带 ext.lock
//!   即可伪造"yomi 装的"所有权证明（原位刷新误删用户目录）。包外
//!   单文件作者无法随包投递，存在与内容都由工具写。
//! - 单文件 = Cargo.lock 同款心智模型：看注册表 `cat` 一个文件，
//!   备份/迁移复制一个文件，没有 per-name 文件蔓延。
//! - 原子重写（tmp + rename）在扩展数量级（个位数~几十个）成本
//!   可忽略；并发由 install/remove 的全局注册表锁串行。
//!
//! 相比 sqlite 记录：没有两份真相的漂移面（list/remove/doctor 全读
//! 文件系统，与 hooks/tools/skills 的"目录即注册表"一致）。正确性
//! 仍不依赖元数据——remove 的兜底（cron 前缀清扫 + 挂载指向判定）
//! 不变，lock 只是让回滚更精确、目录删除有归属证明。
//!
//! hash 口径：包内容 hash 计算**跳过包目录内的 ext.lock**——
//! 0.10.55/56 的存量安装指纹是跳过它算的；包内同名文件（作者自带
//! 或旧版工具写的）只是普通内容，照复制、不解析、不入指纹。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{parse_manifest, ExtManifest, DIR_NAME, LOCK_FILE};

/// 扩展注册表（单文件，Cargo.lock 式）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExtLockfile {
    /// 格式版本，未来结构演进用。
    pub version: u32,
    /// 按名字排序（写入时排序，读取不假设）。
    #[serde(default)]
    pub extensions: Vec<LockEntry>,
}

impl Default for ExtLockfile {
    fn default() -> Self {
        Self {
            version: 1,
            extensions: Vec::new(),
        }
    }
}

/// 一条已装扩展记录（lock 里的 `[[extensions]]`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LockEntry {
    pub name: String,
    /// 安装来源（用户给的原始字符串：GitHub URL 或本地路径）。
    pub source: String,
    /// git 源的 resolved commit sha（本地源为 None）。
    pub rev: Option<String>,
    /// 安装时刻的包内容 hash（blake3 十六进制，不含包内 lock）。
    pub content_hash: String,
    /// 本次安装收编的资源清单（remove 精确回滚用）。
    pub resources: Resources,
    pub installed_at: chrono::DateTime<chrono::Utc>,
}

impl ExtLockfile {
    pub fn get(&self, name: &str) -> Option<&LockEntry> {
        self.extensions.iter().find(|e| e.name == name)
    }

    /// 插入或替换一条记录（按名字），保持名字排序。
    pub fn upsert(&mut self, entry: LockEntry) {
        match self.extensions.iter().position(|e| e.name == entry.name) {
            Some(i) => self.extensions[i] = entry,
            None => self.extensions.push(entry),
        }
        self.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.extensions.len();
        self.extensions.retain(|e| e.name != name);
        self.extensions.len() != before
    }
}

/// 一次安装收编的资源清单（remove 精确回滚用；随 lock 落盘）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Resources {
    /// cron 全名列表（`ext:<名>:<条目>`）。
    pub cron: Vec<String>,
    /// hook 挂载相对路径（`pre_tool_use/50-guard`，不含 hooks/ 前缀）。
    pub hooks: Vec<String>,
    /// bin 挂载文件名（不含 bin/ 前缀）。
    pub bins: Vec<String>,
    /// snippet 文件名。
    pub snippets: Vec<String>,
}

/// install 时写入 lock 的溯源信息（调用面）。
#[derive(Debug, Clone)]
pub struct Provenance {
    /// 用户给的原始来源字符串（GitHub URL 或本地路径）。
    pub source: String,
    /// git 源的 resolved commit sha。
    pub rev: Option<String>,
}

/// 0.10.55/56 的旧版单包 meta（包目录内 ext.lock 的格式）——仅迁移用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct LegacyInstallMeta {
    source: String,
    rev: Option<String>,
    content_hash: String,
    resources: Resources,
    /// 个别手写/截断的 legacy lock 可能缺时间：容错为迁移时刻。
    #[serde(default = "chrono::Utc::now")]
    installed_at: chrono::DateTime<chrono::Utc>,
}

/// 注册表路径：`extensions/ext.lock`。
pub fn lockfile_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR_NAME).join(LOCK_FILE)
}

/// 读注册表。文件缺席或损坏 → 空表（损坏 warn 留痕：等价于全部
/// foreign，目录仍可见，总比注册表消失好）。
pub fn read_lockfile(data_dir: &Path) -> ExtLockfile {
    let path = lockfile_path(data_dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ExtLockfile::default();
    };
    match toml::from_str(&raw) {
        Ok(lf) => lf,
        Err(e) => {
            tracing::warn!(lock = %path.display(), "ext.lock corrupt, treating as empty: {e}");
            ExtLockfile::default()
        }
    }
}

/// 整表原子重写（tmp + rename）：崩溃不留半截 lock。写前按名字排序。
pub fn write_lockfile(data_dir: &Path, lf: &ExtLockfile) -> Result<(), String> {
    let mut lf = lf.clone();
    lf.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    let text = toml::to_string_pretty(&lf).map_err(|e| format!("serialize ext.lock: {e}"))?;
    let path = lockfile_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    // tmp 同目录 + rename：原子替换。
    let tmp = path.with_extension("lock.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", tmp.display()))
}

/// 从旧版包目录内 ext.lock 解析一条记录（0.10.55/56 的存量安装）。
/// 纯读取、不落盘——持久化是 install/remove（持注册表全局锁）的
/// 职责；读路径（list/health）只内存采用，避免无锁整表重写与并发
/// install 的 upsert 互踩。
pub fn legacy_entry(ext_dir: &Path, name: &str) -> Option<LockEntry> {
    let raw = std::fs::read_to_string(ext_dir.join(LOCK_FILE)).ok()?;
    let meta = toml::from_str::<LegacyInstallMeta>(&raw).ok()?;
    Some(LockEntry {
        name: name.to_string(),
        source: meta.source,
        rev: meta.rev,
        content_hash: meta.content_hash,
        resources: meta.resources,
        installed_at: meta.installed_at,
    })
}

/// 一个已装扩展 = 其 manifest + 注册表里的条目（可缺：用户手放的
/// 目录——list 展示为 foreign，remove 拒绝）。legacy（0.10.55/56 的
/// in-dir lock）在读取时内存采用，不写注册表。
#[derive(Debug, Clone)]
pub struct InstalledExt {
    pub manifest: ExtManifest,
    pub meta: Option<LockEntry>,
    pub dir: PathBuf,
}

impl InstalledExt {
    pub fn name(&self) -> &str {
        &self.manifest.ext.name
    }

    /// 本次安装挂载的相对 `data_dir` 路径集合（`hooks/...` / `bin/...`）。
    pub fn mount_paths(&self) -> Vec<String> {
        let meta = match &self.meta {
            Some(m) => m,
            None => return Vec::new(),
        };
        let mut out: Vec<String> = meta
            .resources
            .hooks
            .iter()
            .map(|s| format!("hooks/{s}"))
            .collect();
        out.extend(meta.resources.bins.iter().map(|s| format!("bin/{s}")));
        out
    }
}

/// 解析单个已装目录：ext.toml（[ext]，作者 manifest）+ 注册表条目。
/// manifest 损坏时返回 Err（调用方归 placeholder → foreign）。
pub fn read_installed(data_dir: &Path, ext_dir: &Path) -> Result<InstalledExt, String> {
    let manifest = parse_manifest(ext_dir).map_err(|e| e.to_string())?;
    let name = manifest.ext.name.clone();
    // 注册表条目；存量（0.10.55/56）in-dir lock 读取时内存采用，
    // 持久化留给 install/remove 的持锁收养——读路径不做无锁整表重写
    // （会与并发 install 的 upsert 互踩）。
    let meta = read_lockfile(data_dir)
        .get(&name)
        .cloned()
        .or_else(|| legacy_entry(ext_dir, &name));
    Ok(InstalledExt {
        manifest,
        meta,
        dir: ext_dir.to_path_buf(),
    })
}

/// 扫描全部已装扩展：extensions/*/ext.toml（ext.lock 注册表文件本身
/// 跳过），目录名字典序。目录在但 ext.toml 不可读的 → 归 broken
/// （meta=None、manifest 缺省由调用方展示），不跳过——健康检查要
/// 看见它。
pub async fn list_installed(data_dir: &Path) -> Vec<InstalledExt> {
    let root = data_dir.join(DIR_NAME);
    let Ok(mut rd) = tokio::fs::read_dir(&root).await else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        // 注册表文件不是扩展目录。
        if name == LOCK_FILE {
            continue;
        }
        if tokio::fs::metadata(entry.path()).await.is_err() {
            // 破损 symlink（如手工挪走源）：跳过但留痕——doctor 的
            // 健康检查至少能在日志里追到这个目录。
            tracing::warn!(dir = %entry.path().display(), "installed extension unreadable (broken symlink?); skipped");
            continue;
        }
        dirs.push(entry.path());
    }
    dirs.sort();

    let mut out = Vec::new();
    for dir in dirs {
        match read_installed(data_dir, &dir) {
            Ok(installed) => out.push(installed),
            Err(e) => {
                tracing::warn!(dir = %dir.display(), "installed extension unreadable: {e}");
                // broken 占位：名字取目录名，meta=None。
                if let Ok(manifest) = foreign_placeholder(&dir) {
                    out.push(InstalledExt {
                        manifest,
                        meta: None,
                        dir,
                    });
                }
            }
        }
    }
    out
}

/// broken 目录的占位 manifest（只有名字能确定）。
fn foreign_placeholder(dir: &Path) -> Result<ExtManifest, String> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // 复用校验规则能过才用（目录名非法时跳过该条目）。
    let toml_text =
        format!("[ext]\nname = \"{name}\"\nversion = \"0\"\ndescription = \"(unreadable)\"\n");
    toml::from_str(&toml_text).map_err(|e| format!("placeholder: {e}"))
}

#[cfg(test)]
#[path = "installed_test.rs"]
mod tests;
