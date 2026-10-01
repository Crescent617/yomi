//! 已装扩展的元数据：install 复制包后往目录里盖 `ext.lock`（等价
//! Cargo.lock 对 Cargo.toml——ext.toml 是作者的 manifest，原封不动），
//! 目录本身就是注册表。
//!
//! 相比 sqlite 记录：没有两份真相的漂移面（list/remove/doctor 全读
//! 文件系统，与 hooks/tools/skills 的"目录即注册表"一致）；重装自然
//! 重写 lock。正确性仍不依赖元数据——remove 的兜底（cron 前缀清扫
//! + 挂载指向判定）不变，lock 只是让回滚更精确。
//!
//! ext.lock 在包内容 hash 计算**之后**写入，且 hash 算法跳过它：
//! hash 覆盖包内容（含 `[ext]` 段）用于本地改动侦测；lock 是我们的、
//! 每次重装重写，不入 hash。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{parse_manifest, ExtManifest, DIR_NAME, LOCK_FILE};

/// 一次安装收编的资源清单（remove 精确回滚用；随 ext.lock 落盘）。
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

/// 安装来源溯源（写入 ext.lock）。
#[derive(Debug, Clone)]
pub struct Provenance {
    /// 用户给的原始来源字符串（GitHub URL 或本地路径）。
    pub source: String,
    /// git 源的 resolved commit sha。
    pub rev: Option<String>,
}

/// install 时写入 ext.lock 的溯源信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallMeta {
    /// 安装来源（用户给的原始字符串：GitHub URL 或本地路径）。
    pub source: String,
    /// git 源的 resolved commit sha（本地源为 None）。
    pub rev: Option<String>,
    /// 安装时刻的包内容 hash（blake3 十六进制，不含本段）。
    pub content_hash: String,
    /// 本次安装收编的资源清单（remove 精确回滚用）。
    pub resources: Resources,
    pub installed_at: chrono::DateTime<chrono::Utc>,
}

/// 一个已装扩展 = 其 manifest + ext.lock 元数据（可缺：用户手
/// 放的目录——list 展示为 foreign，remove 拒绝）。
#[derive(Debug, Clone)]
pub struct InstalledExt {
    pub manifest: ExtManifest,
    pub meta: Option<InstallMeta>,
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

/// 把安装溯源写进已装目录的 `ext.lock`（hash 计算之后调用）。ext.toml
/// 是作者的 manifest，**原封不动**——lock 独立成文件，等价 Cargo.lock
/// 对 Cargo.toml。整篇重写（不留旧内容），并发/重试下结果确定。
pub fn write_install_meta(
    ext_dir: &Path,
    content_hash: &str,
    provenance: &Provenance,
    resources: &Resources,
) -> Result<(), String> {
    let meta = InstallMeta {
        source: provenance.source.clone(),
        rev: provenance.rev.clone(),
        content_hash: content_hash.to_string(),
        resources: resources.clone(),
        installed_at: chrono::Utc::now(),
    };
    let text = toml::to_string_pretty(&meta).map_err(|e| format!("serialize install meta: {e}"))?;
    // tmp + rename 原子写：崩溃不留半截 lock（半截 = meta None = foreign =
    // 重装被 occupied 挡，要手工清）。tmp 以 . 开头，扫描器跳过。
    let tmp = ext_dir.join(".ext.lock.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, ext_dir.join(LOCK_FILE))
        .map_err(|e| format!("rename {}: {e}", tmp.display()))
}

/// 解析单个已装目录：ext.toml（[ext]，作者 manifest）+ 可选 ext.lock
/// （install 盖的溯源/资源清单；缺席 = foreign 目录）。manifest 本身
/// 损坏时返回 Err（调用方归类为 unreadable）。
pub fn read_installed(ext_dir: &Path) -> Result<InstalledExt, String> {
    let manifest = parse_manifest(ext_dir).map_err(|e| e.to_string())?;
    // ext.lock 缺席不算错（foreign 目录）；存在但损坏按无 meta 处理
    // （目录仍可见，健康检查报 foreign——总比整目录消失好）。
    let meta = std::fs::read_to_string(ext_dir.join(LOCK_FILE))
        .ok()
        .and_then(|raw| toml::from_str::<InstallMeta>(&raw).ok());
    Ok(InstalledExt {
        manifest,
        meta,
        dir: ext_dir.to_path_buf(),
    })
}

/// 扫描全部已装扩展：extensions/*/ext.toml，目录名字典序。
/// 目录在但 ext.toml 不可读的 → 归 broken（meta=None、manifest 缺省由
/// 调用方展示），不跳过——健康检查要看见它。
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
        match read_installed(&dir) {
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
