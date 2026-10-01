//! SP snippet 扫描：extensions/*/snippets/*.md 拼进 system prompt。
//!
//! 约定式资源（无 manifest 条目）：扩展名字典序 → snippet 文件名序。
//! 跟随 symlink（extensions/<名> 本身是 symlink），破损源静默跳过
//! （list 的健康状态负责暴露）。常规路径走 [`SnippetLoader`] 的 60s
//! TTL 缓存（对齐 skills 的 `SkillLoader`：spawn 频率下不做每次全量
//! IO），TTL 即生效延迟上限。
//!
//! 安全边界：snippet 只读常规文件（FIFO 会卡死 tokio blocking 线程）；
//! 单文件 16KB 上限（限长读），单扩展 64 个上限。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::{DIR_NAME, SNIPPETS_DIR};

/// 单个 snippet：进 prompt 的最小单元。
#[derive(Debug, Clone)]
pub struct Snippet {
    /// 扩展名（`# Extension: <名>` 标题）。
    pub ext: String,
    /// snippet 文件名（排序键，展示/排障用）。
    pub file: String,
    /// 原文（trim 由拼装侧做；超长的截断带标记）。
    pub content: String,
}

/// 单文件内容上限。
pub const SNIPPET_MAX_BYTES: usize = 16 * 1024;
/// 单扩展 snippet 数上限（防每次 spawn 的线性 IO 膨胀）。
pub const SNIPPETS_PER_EXT_MAX: usize = 64;
/// 扫描缓存 TTL（对齐 skills 的 `SCAN_TTL`）。
pub const SNIPPET_SCAN_TTL: Duration = Duration::from_mins(1);

/// `data_dir` → 扫描结果的进程级缓存：TTL 60s + moka 单飞（并发 spawn
/// 合并为一次扫描）。
#[derive(Clone)]
pub struct SnippetLoader {
    cache: moka::future::Cache<PathBuf, Arc<Vec<Snippet>>>,
}

impl Default for SnippetLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// 进程级共享 loader（daemon 单进程；每 spawn 新建会使缓存失效）。
static GLOBAL: std::sync::LazyLock<SnippetLoader> = std::sync::LazyLock::new(SnippetLoader::new);

impl SnippetLoader {
    /// 共享实例：prompt 装配每 spawn 调用，必须复用同一缓存。
    pub fn global() -> &'static SnippetLoader {
        &GLOBAL
    }

    pub fn new() -> Self {
        Self {
            cache: moka::future::Cache::builder()
                .time_to_live(SNIPPET_SCAN_TTL)
                .build(),
        }
    }

    /// 全部已安装扩展的 snippets（扩展名字典序 → 文件名序）。
    pub async fn load(&self, data_dir: &Path) -> Arc<Vec<Snippet>> {
        let key = data_dir.to_path_buf();
        self.cache
            .get_with(key.clone(), async move { Arc::new(scan(&key).await) })
            .await
    }
}

/// 无缓存的一次性扫描（测试用；常规路径走 [`SnippetLoader::load`]）。
pub async fn load_snippets(data_dir: &Path) -> Vec<Snippet> {
    scan(data_dir).await
}

async fn scan(data_dir: &Path) -> Vec<Snippet> {
    let root = data_dir.join(DIR_NAME);
    let Ok(mut exts) = tokio::fs::read_dir(&root).await else {
        return Vec::new();
    };
    let mut ext_dirs: Vec<(String, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = exts.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        // 跟随 symlink（破损源：metadata 失败，跳过）。
        if tokio::fs::metadata(entry.path()).await.is_err() {
            tracing::debug!(ext = %name, "extension dir unreadable; skipping snippets");
            continue;
        }
        ext_dirs.push((name, entry.path()));
    }
    ext_dirs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = Vec::new();
    for (ext, dir) in ext_dirs {
        let snippets_dir = dir.join(SNIPPETS_DIR);
        let Ok(mut files) = tokio::fs::read_dir(&snippets_dir).await else {
            continue;
        };
        let mut names: Vec<String> = Vec::new();
        while let Ok(Some(f)) = files.next_entry().await {
            let name = f.file_name().to_string_lossy().into_owned();
            if name.to_ascii_lowercase().ends_with(".md") && !name.starts_with('.') {
                names.push(name);
            }
        }
        names.sort();
        if names.len() > SNIPPETS_PER_EXT_MAX {
            tracing::warn!(ext = %ext, count = names.len(), "extension has more than {SNIPPETS_PER_EXT_MAX} snippets; excess ignored");
            names.truncate(SNIPPETS_PER_EXT_MAX);
        }
        for file in names {
            let path = snippets_dir.join(&file);
            let content = match read_bounded(&path).await {
                Ok(Some(content)) => content,
                Ok(None) => continue, // 非常规文件/读失败已留日志
                Err(e) => {
                    tracing::warn!(path = %path.display(), "snippet read failed: {e}");
                    continue;
                }
            };
            out.push(Snippet {
                ext: ext.clone(),
                file,
                content,
            });
        }
    }
    out
}

/// 限长读：最多 max+1 字节（判断截断），只接受常规文件（FIFO 会阻塞
/// tokio blocking 线程）。`Ok(None)` = 跳过（已留日志）。
async fn read_bounded(path: &Path) -> std::io::Result<Option<String>> {
    use tokio::io::AsyncReadExt as _;

    let md = match tokio::fs::metadata(path).await {
        Ok(md) => md,
        Err(e) => {
            tracing::debug!(path = %path.display(), "snippet metadata failed: {e}");
            return Ok(None);
        }
    };
    if !md.is_file() {
        tracing::warn!(path = %path.display(), "snippet is not a regular file; skipped");
        return Ok(None);
    }
    let file = tokio::fs::File::open(path).await?;
    let mut buf = Vec::new();
    file.take(SNIPPET_MAX_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .await?;
    Ok(Some(truncate_utf8(&buf, SNIPPET_MAX_BYTES)))
}

/// UTF-8 安全截断（截断点回退到 char 边界；截过时加标记）。
fn truncate_utf8(bytes: &[u8], max: usize) -> String {
    if bytes.len() <= max {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut end = max;
    while end > 0 && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    format!("{}…\n\n(truncated)", String::from_utf8_lossy(&bytes[..end]))
}

#[cfg(test)]
#[path = "snippets_test.rs"]
mod tests;
