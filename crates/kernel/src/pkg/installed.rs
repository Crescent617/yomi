//! 已装扩展的元数据：npm package.json 玩法——install 复制包后往
//! `extensions/<名>/ext.toml` 追加 `[install]` 段，目录本身就是注册表。
//!
//! 相比 sqlite 记录：没有两份真相的漂移面（list/remove/doctor 全读
//! 文件系统，与 hooks/tools/skills 的"目录即注册表"一致）；重装自然
//! 重写元数据。正确性仍不依赖元数据——remove 的兜底（cron 前缀清扫
//! + 挂载指向判定）不变，元数据只是让回滚更精确。
//!
//! `[install]` 段在包内容 hash 计算**之后**写入：hash 覆盖包内容
//! （含 `[ext]` 段）用于本地改动侦测；`[install]` 是我们的、每次重装
//! 重写，不入 hash。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{parse_manifest, ExtManifest, DIR_NAME, MANIFEST_FILE};

/// 一次安装收编的资源清单（remove 精确回滚用；随 `[install]` 段落盘）。
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

/// 安装来源溯源（写入 `[install]` 段）。
#[derive(Debug, Clone)]
pub struct Provenance {
    /// 用户给的原始来源字符串（GitHub URL 或本地路径）。
    pub source: String,
    /// git 源的 resolved commit sha。
    pub rev: Option<String>,
}

/// install 时写入 ext.toml 的 `[install]` 段。
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

/// 一个已装扩展 = 其 manifest + [install] 元数据（元数据可缺：用户手
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

/// 一次安装溯源信息的序列化信封：包一层才带 `[install]` 表头——直接
/// 序列化 `InstallMeta` 只会得到散在顶层的 key，追加进 ext.toml 时与
/// [ext] 段串台。
#[derive(serde::Serialize)]
struct InstallSection<'a> {
    install: &'a InstallMeta,
}

/// 把 `[install]` 段写进已装目录的 ext.toml（hash 计算之后调用）。
/// 整篇重写而非追加：先剥离包内可能自带的同名表（防伪造归属证明/
/// 重复表损坏 TOML），再落新段——并发/重试下结果确定。
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
    // 用信封序列化（见 InstallSection）。
    let section = toml::to_string_pretty(&InstallSection { install: &meta })
        .map_err(|e| format!("serialize install meta: {e}"))?;
    let path = ext_dir.join(MANIFEST_FILE);
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    // manifest 已在此前的 parse_manifest 校验过，这里解析失败属 IO 竞
    // 态——按原样保留正文 + 新段照旧尝试，不因剥离失败丢安装记录。
    let mut body = match raw.parse::<toml::Table>() {
        Ok(mut t) => {
            t.remove("install");
            toml::to_string_pretty(&t).unwrap_or_else(|_| raw.clone())
        }
        Err(_) => raw.clone(),
    };
    body.push('\n');
    body.push_str(&section);
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

/// 解析单个已装目录的 ext.toml（[ext] + 可选 [install]）。manifest
/// 本身损坏时返回 Err（调用方归类为 broken）。
pub fn read_installed(ext_dir: &Path) -> Result<InstalledExt, String> {
    let path = ext_dir.join(MANIFEST_FILE);
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let manifest = parse_manifest(ext_dir).map_err(|e| e.to_string())?;
    // [install] 段缺席不算错（foreign 目录）。整篇文档用 Table 解析
    // （Value 的 FromStr 只收单个值，整篇会报 "unexpected content"）。
    let meta = raw
        .parse::<toml::Table>()
        .ok()
        .and_then(|mut t| t.remove("install"))
        .and_then(|v| InstallMeta::deserialize(v).ok());
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
