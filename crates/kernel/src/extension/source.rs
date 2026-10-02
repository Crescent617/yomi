//! 扩展包来源：GitHub URL（默认玩法，同 nvim 插件 / npx skills）或本
//! 地目录（开发形态）。两种形态统一 copy 进 `extensions/<名>`——
//! install 永远是"取货 + 复制"，重装即更新，没有 symlink 模式的
//! 状态分叉（broken source、模式切换、记录与磁盘不一致）。
//!
//! URL 形态：`owner/repo[/子目录][@ref]`，也收完整
//! `https://github.com/owner/repo` URL（`.git` 后缀容忍）。`@ref` 为
//! 分支/tag 时 `git clone --depth 1 --branch`；为 40 位 sha 时全量
//! clone + checkout。

use std::path::{Path, PathBuf};

use super::ExtError;

/// Git clone 超时（网络慢/挂时别让 RPC 路径无限等）。
const CLONE_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(3);

/// 已解析的安装来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtSource {
    /// 本地目录（开发/私有包；同样复制进 extensions/，编辑后重装生效）。
    Local(PathBuf),
    /// git 仓。`url` 是规范化后的 clone URL；`subdir` 是包在仓内的
    /// 路径（多扩展共仓时用）；`ref` 是分支/tag/sha（None = 默认分支）。
    Git {
        url: String,
        subdir: Option<String>,
        ref_: Option<String>,
    },
}

/// 解析安装来源字符串。已存在的目录走 Local；否则按 GitHub 简写或
/// https URL 解析。两者都不像 → Invalid。
pub fn parse_source(s: &str) -> Result<ExtSource, ExtError> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(ExtError::Invalid("empty source".to_string()));
    }
    // 本地目录优先：开发期 "owner/repo" 形式的相对路径若恰好存在同名
    // 目录，按本地处理（显式 ./ 前缀或绝对路径无歧义）。
    let p = Path::new(trimmed);
    if p.is_dir() {
        return Ok(ExtSource::Local(p.canonicalize().map_err(|e| {
            ExtError::Invalid(format!("canonicalize {}: {e}", p.display()))
        })?));
    }
    parse_git(trimmed)
}

/// `owner/repo[/subdir][@ref]` 或 <https://github.com/owner/repo>[...]。
fn parse_git(s: &str) -> Result<ExtSource, ExtError> {
    let mut rest = s.strip_prefix("https://github.com/").unwrap_or(s);
    rest = rest.strip_prefix("github.com/").unwrap_or(rest);
    rest = rest.trim_end_matches('/');
    // @ref 取最后一个 @ 之后的全部（分支名常带 /：feature/x、hotfix/y）；
    // 路径段本身不允许 @，无歧义。先切 ref 再剥 .git 后缀——
    // `owner/repo.git@main` 的 .git 属于 repo 名，剥在 ref 切分之后才
    // 不会把 r.git 当仓库名（clone URL 拼成 r.git.git 必然失败）。
    let (rest, ref_) = match rest.rsplit_once('@') {
        Some((head, tail)) if !tail.is_empty() && !head.is_empty() => {
            (head.trim_end_matches(".git"), Some(tail.to_string()))
        }
        _ => (rest.trim_end_matches(".git"), None),
    };
    let segments: Vec<&str> = rest.split('/').filter(|seg| !seg.is_empty()).collect();
    if segments.len() < 2 {
        return Err(ExtError::Invalid(format!(
            "source '{s}': not a local directory and not a GitHub repo (expected owner/repo[/subdir][@ref])"
        )));
    }
    for seg in &segments {
        // 防路径穿越与怪字符：段名只允许 GitHub 兼容字符集。
        if *seg == "." || *seg == ".." {
            return Err(ExtError::Invalid(format!(
                "source '{s}': bad path segment '{seg}'"
            )));
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(ExtError::Invalid(format!(
                "source '{s}': bad path segment '{seg}'"
            )));
        }
    }
    let owner = segments[0];
    let repo = segments[1];
    let subdir = if segments.len() > 2 {
        Some(segments[2..].join("/"))
    } else {
        None
    };
    Ok(ExtSource::Git {
        url: format!("https://github.com/{owner}/{repo}.git"),
        subdir,
        ref_,
    })
}

/// 取货：Git 来源 clone 到临时目录，返回 (临时目录句柄, 包根路径,
/// resolved commit sha)。临时目录句柄由调用方持有——install 复制完
/// 即随作用域清理，网络失败不脏任何槽位。本地来源返回其路径，rev 为
/// None（内容 hash 仍可溯源）。
pub async fn fetch_source(
    src: &ExtSource,
) -> Result<(Option<tempfile::TempDir>, PathBuf, Option<String>), ExtError> {
    match src {
        ExtSource::Local(dir) => Ok((None, dir.clone(), None)),
        ExtSource::Git { url, subdir, ref_ } => {
            let tmp = tempfile::tempdir()
                .map_err(|e| ExtError::Fetch(format!("create temp dir: {e}")))?;
            let dest = tmp.path().join("repo");
            clone(url, ref_.as_deref(), &dest).await?;
            // clone 落盘的总量闸：hash 那道闸只看包内容（还跳过 .git），
            // 挡不住"仓里塞 10GB pack"把临时盘灌满——metadata 级 walk
            // 把 .git 也算上，超限即弃（TempDir 随 Err 清理）。
            clone_size_guard(&dest)?;
            let rev = resolve_head(&dest).await?;
            let root = match subdir {
                Some(sub) => {
                    let root = dest.join(sub);
                    if !root.is_dir() {
                        return Err(ExtError::Fetch(format!(
                            "subdirectory '{sub}' not found in {url}"
                        )));
                    }
                    root
                }
                None => dest,
            };
            Ok((Some(tmp), root, rev))
        }
    }
}

/// clone 落盘总量上限：包的持久化内容由 install 的逐文件/总量闸管，
/// 这里只管临时 clone（含 .git）别把盘灌满。
const CLONE_TOTAL_MAX_BYTES: u64 = 512 * 1024 * 1024;
const CLONE_FILE_MAX: usize = 50_000;

/// metadata 级递归统计：总字节与文件数任一超限即拒。不读内容，
/// 对 .git pack 也生效。
fn clone_size_guard(dir: &Path) -> Result<(), ExtError> {
    fn walk(dir: &Path, total: &mut u64, count: &mut usize) -> Result<(), ExtError> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk(&entry.path(), total, count)?;
            } else if ft.is_file() {
                *count += 1;
                *total += entry.metadata()?.len();
                if *total > CLONE_TOTAL_MAX_BYTES {
                    return Err(ExtError::Fetch(format!(
                        "cloned repo exceeds {CLONE_TOTAL_MAX_BYTES} bytes"
                    )));
                }
                if *count > CLONE_FILE_MAX {
                    return Err(ExtError::Fetch(format!(
                        "cloned repo exceeds {CLONE_FILE_MAX} files"
                    )));
                }
            }
        }
        Ok(())
    }
    let mut total = 0u64;
    let mut count = 0usize;
    walk(dir, &mut total, &mut count)
}

/// clone 后的 HEAD sha（安装的精确版本，进安装记录）。
async fn resolve_head(dest: &Path) -> Result<Option<String>, ExtError> {
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dest)
        .output()
        .await
        .map_err(|e| ExtError::Fetch(format!("git rev-parse: {e}")))?;
    if !out.status.success() {
        return Ok(None); // 记录可缺，别因溯源失败挂掉 install
    }
    Ok(Some(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

/// git clone：分支/tag 走 shallow；sha 走全量 + checkout。
async fn clone(url: &str, ref_: Option<&str>, dest: &Path) -> Result<(), ExtError> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("clone").arg("--quiet").kill_on_drop(true);
    let is_sha = ref_.is_some_and(|r| r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit()));
    if let (Some(r), false) = (ref_, is_sha) {
        cmd.args(["--depth", "1", "--branch", r]);
    }
    cmd.arg(url).arg(dest);
    crate::utils::env::inject_child_env(&mut cmd, None, None);

    let output = tokio::time::timeout(CLONE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            ExtError::Fetch(format!(
                "git clone timed out after {CLONE_TIMEOUT:?}: {url} (killed)"
            ))
        })?
        .map_err(|e| ExtError::Fetch(format!("git clone spawn failed: {e}")))?;
    if !output.status.success() {
        return Err(ExtError::Fetch(format!(
            "git clone failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if let (Some(sha), true) = (ref_, is_sha) {
        let mut checkout = tokio::process::Command::new("git");
        checkout
            .args(["checkout", "--quiet", sha])
            .current_dir(dest);
        let out = checkout
            .output()
            .await
            .map_err(|e| ExtError::Fetch(format!("git checkout: {e}")))?;
        if !out.status.success() {
            return Err(ExtError::Fetch(format!(
                "git checkout {sha} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "source_test.rs"]
mod tests;
