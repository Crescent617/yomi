//! install/remove：扩展包的物化与回收。
//!
//! 规则全文见 `docs/design/ext-packages.md` 与模块根 doc。承重墙：
//! 所有权 = symlink 文本目标相等；cron 全名 `ext:<名>:` 前缀；
//! install 纯 additive；remove 只删能证明属于自己的东西。

use std::path::Path;
use std::sync::Arc;

use super::store::ExtInstall;
use super::{cron_name, PkgError, BIN_DIR, DIR_NAME, HOOKS_DIR, SNIPPETS_DIR};
use crate::cron::{CronAction, CronSessionTemplate, CronStore};
use crate::permission::Level;

/// 一次安装的完整报告（wire 序列化给 CLI 展示，同时进安装记录）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallReport {
    pub name: String,
    pub version: String,
    /// cron 收养结果：`created=false` = 已存在未动（ensure 语义）。
    pub cron: Vec<CronAdoptReport>,
    pub hooks: Vec<MountReport>,
    pub bins: Vec<MountReport>,
    /// 包内 snippet 文件名（约定式资源，不物化）。
    pub snippets: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CronAdoptReport {
    pub name: String,
    pub created: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MountReport {
    /// 相对 `data_dir` 的挂载路径（`hooks/pre_tool_use/50-guard` / `bin/recall`）。
    pub path: String,
    pub status: MountStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MountStatus {
    /// 本次新建 symlink。
    Linked,
    /// 已指向本包，跳过（幂等重跑 / 崩溃恢复路径）。
    Already,
}

/// 一次卸载的完整报告。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoveReport {
    pub name: String,
    pub cron_removed: Vec<String>,
    /// 指向不符被留下的挂载（用户换过槽位内容）。
    pub mounts_left: Vec<String>,
    pub mounts_removed: Vec<String>,
    /// `extensions/<名>` 是否已删除（records-missing 路径下包目录可能已不在）。
    pub ext_dir_removed: bool,
    /// 安装记录是否已删除（Kernel 侧 store 成功才为 true）。
    pub record_deleted: bool,
}

/// 收编 + 挂载 + 收养 cron。幂等：重复调用收敛到同一终态。
///
/// `source` 是包根目录（含 ext.toml）；`copy` 为 true 时实体复制进
/// extensions/（默认 symlink，源仓 git pull 即更新）。
pub async fn install(
    data_dir: &Path,
    source: &Path,
    copy: bool,
    cron_store: &Arc<dyn CronStore>,
    config_auto_approve: Level,
    // 调用方预先解析校验好的 manifest。**name 以它为准**——调用方查
    // 安装记录用的必须是同一个 manifest，否则两次 parse 之间文件被
    // 换名会导致 copy 刷新拿着 A 的记录删 B 槽位的目录（TOCTOU）。
    manifest: &super::ExtManifest,
    // 已安装记录的 mode（`Some("copy")` 时 copy 重装可原位刷新；None =
    // 无记录，实体槽位一律拒绝）。
    record_mode: Option<&str>,
) -> Result<InstallReport, PkgError> {
    let source = source
        .canonicalize()
        .map_err(|e| PkgError::Invalid(format!("package dir {}: {e}", source.display())))?;
    if !source.is_dir() {
        return Err(PkgError::Invalid(format!(
            "package dir {} is not a directory",
            source.display()
        )));
    }
    let name = manifest.ext.name.clone();
    let ext_dir = data_dir.join(DIR_NAME).join(&name);

    adopt(&ext_dir, &source, copy, copy && record_mode == Some("copy")).await?;

    // 挂载 hooks 与 bin：先全部走完收集冲突，有冲突整体报错——
    // 已建部分不用回滚，所有权规则保证重跑 install 收敛。
    let mut conflicts: Vec<String> = Vec::new();
    let mut hooks = Vec::new();
    for (point, entry, is_dir) in scan_hook_entries(&ext_dir).await {
        let rel = format!("{HOOKS_DIR}/{point}/{entry}");
        let status = mount(
            &data_dir.join(&rel),
            &ext_dir.join(&rel),
            is_dir,
            &mut conflicts,
        )
        .await?;
        hooks.push(MountReport { path: rel, status });
    }
    let mut bins = Vec::new();
    for (file, exec) in scan_bin_files(&ext_dir).await {
        if !exec {
            tracing::warn!(bin = %file, ext = %name, "bin entry lacks exec bit; linking anyway (chmod to enable)");
        }
        let rel = format!("{BIN_DIR}/{file}");
        let status = mount(
            &data_dir.join(&rel),
            &ext_dir.join(&rel),
            false,
            &mut conflicts,
        )
        .await?;
        bins.push(MountReport { path: rel, status });
    }
    if !conflicts.is_empty() {
        return Err(PkgError::Conflict(conflicts.join("; ")));
    }

    // cron 收养（ensure：缺才建，已存在不动）。
    let mut cron = Vec::new();
    for entry in &manifest.cron {
        let full = cron_name(&name, &entry.name);
        let content = entry.resolve_message(&source)?;
        let input = crate::cron::CreateCronJobInput {
            name: full.clone(),
            schedule: entry.schedule.clone(),
            action: CronAction::SendMessage {
                session_id: None,
                content,
                session_template: entry.work_dir.as_ref().map(|dir| CronSessionTemplate {
                    working_dir: Some(dir.clone()),
                    project_id: None,
                    auto_approve_level: None,
                }),
            },
            max_runs: None,
            expires_at: None,
            precheck: entry.precheck.clone(),
        };
        let outcome =
            crate::cron::create_cron_job(cron_store, None, input, config_auto_approve).await?;
        cron.push(CronAdoptReport {
            name: full,
            created: outcome.created,
        });
    }

    let snippets = list_snippets(&ext_dir).await;

    Ok(InstallReport {
        name,
        version: manifest.ext.version.clone(),
        cron,
        hooks,
        bins,
        snippets,
    })
}

/// 卸载：cron 前缀清扫 → 摘挂载（指向判定）→ 删 extensions/<名>。
/// 记录缺失时退化为扫包目录 + cron 前缀（正确性不依赖记录）。
pub async fn remove(
    data_dir: &Path,
    name: &str,
    cron_store: &Arc<dyn CronStore>,
    record: Option<&ExtInstall>,
) -> Result<RemoveReport, PkgError> {
    // 名字与 install 同一规则校验：`../` 之类的穿越在此被硬拒（否则
    // join 后 remove_dir_all 会删到数据目录外）。
    if !super::manifest::valid_ext_name(name) {
        return Err(PkgError::Invalid(format!(
            "extension name '{name}' invalid: letter first, [a-z0-9-] only, ≤32 chars"
        )));
    }
    let ext_dir = data_dir.join(DIR_NAME).join(name);

    // cron：按前缀清扫（记录里的名单是提示，前缀才是真相——防止记录
    // 漏掉手动 ensure 进来的同名 job）。store 层前缀查询，无分页漏删
    // 窗口。
    let prefix = format!("ext:{name}:");
    let mut cron_removed = Vec::new();
    for job in cron_store.list_by_prefix(&prefix, 10_000).await? {
        if cron_store.delete(&job.id).await? {
            cron_removed.push(job.name);
        }
    }

    // 挂载名单：记录优先；包目录还在则并集（记录缺失的退化路径）。
    // scan 可能因源被删而失败——静默用记录名单即可。
    let mut mount_rels: Vec<String> = record.map(|r| r.mount_paths()).unwrap_or_default();
    if let Ok(list) = try_scan_hook_rels(&ext_dir).await {
        for rel in list {
            if !mount_rels.contains(&rel) {
                mount_rels.push(rel);
            }
        }
    }
    if let Ok(list) = try_scan_bin_rels(&ext_dir).await {
        for rel in list {
            if !mount_rels.contains(&rel) {
                mount_rels.push(rel);
            }
        }
    }
    mount_rels.sort();

    let mut mounts_removed = Vec::new();
    let mut mounts_left = Vec::new();
    for rel in mount_rels {
        let link = data_dir.join(&rel);
        let expected = ext_dir.join(&rel);
        match tokio::fs::symlink_metadata(&link).await {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(md) if md.file_type().is_symlink() => {
                let cur = tokio::fs::read_link(&link).await?;
                if cur == expected {
                    tokio::fs::remove_file(&link).await?;
                    mounts_removed.push(rel);
                } else {
                    tracing::warn!(link = %link.display(), target = %cur.display(), "mount slot repointed; leaving it");
                    mounts_left.push(rel);
                }
            }
            Ok(_) => {
                tracing::warn!(link = %link.display(), "mount slot replaced with a non-symlink; leaving it");
                mounts_left.push(rel);
            }
        }
    }

    // 最后删包目录本身（symlink 或实体复制）。
    let ext_dir_removed = match tokio::fs::symlink_metadata(&ext_dir).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
        Ok(md) if md.file_type().is_symlink() => {
            tokio::fs::remove_file(&ext_dir).await?;
            true
        }
        // 实体目录：只有记录证明是我们 copy 的才删；用户把自己的目录
        // 放进槽位时留下并 warn（设计：全程只动能证明属于自己的东西）。
        Ok(_) if record.is_some_and(|r| r.mode == "copy") => {
            tokio::fs::remove_dir_all(&ext_dir).await?;
            true
        }
        Ok(_) => {
            tracing::warn!(dir = %ext_dir.display(), "extensions slot is a non-symlink dir not owned by this install record; leaving it");
            false
        }
    };

    Ok(RemoveReport {
        name: name.to_string(),
        cron_removed,
        mounts_left,
        mounts_removed,
        ext_dir_removed,
        record_deleted: false, // Kernel 侧 store 删除成功后置位
    })
}

/// `extensions/<名>` 槽位收编：空则 symlink/copy；已指向同一源则幂等；
/// 实体目录仅当 `allow_replace`（copy 重装、记录证明是我们复制的）才
/// 原位刷新；其他一律拒绝（绝不覆盖用户目录）。
async fn adopt(
    ext_dir: &Path,
    source: &Path,
    copy: bool,
    allow_replace: bool,
) -> Result<(), PkgError> {
    match tokio::fs::symlink_metadata(ext_dir).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = ext_dir.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            if copy {
                copy_dir(source, ext_dir).await
            } else {
                os_symlink(source, ext_dir, true).await?;
                Ok(())
            }
        }
        Err(e) => Err(e.into()),
        Ok(md) if md.file_type().is_symlink() => {
            let cur = tokio::fs::read_link(ext_dir).await?;
            if cur != source {
                return Err(PkgError::Conflict(format!(
                    "extensions slot {} already points to {} (remove it first)",
                    ext_dir.display(),
                    cur.display()
                )));
            }
            if copy {
                // 模式切换（symlink → copy）：换掉 symlink、实体复制。
                // 删 symlink 不碰用户数据（内容本就等于源），记录与
                // 磁盘状态保持一致。
                tokio::fs::remove_file(ext_dir).await?;
                copy_dir(source, ext_dir).await
            } else {
                Ok(())
            }
        }
        Ok(_) => {
            if allow_replace {
                copy_refresh(source, ext_dir).await
            } else {
                Err(PkgError::Conflict(format!(
                    "extensions slot {} is occupied by a non-symlink (remove it first)",
                    ext_dir.display()
                )))
            }
        }
    }
}

/// copy 原位刷新：先复制到隐藏临时目录再 swap，避免"先删旧再拷新"
/// 的中途窗口（那时 `ext_dir` 消失，全部挂载悬空且旧版本已丢）。
/// 临时目录名以 `.` 开头——hook/tool 扫描器跳过隐藏项，不会被半成品
/// 内容看见。swap 本身的微小缺失窗由扫描器对 broken symlink 的
/// 跳过语义兜住（fail-open）。
async fn copy_refresh(source: &Path, ext_dir: &Path) -> Result<(), PkgError> {
    let tmp = ext_dir.parent().unwrap_or(ext_dir).join(format!(
        ".{}.tmp",
        ext_dir.file_name().unwrap_or_default().to_string_lossy()
    ));
    tokio::fs::remove_dir_all(&tmp).await.ok();
    if let Err(e) = copy_dir(source, &tmp).await {
        tokio::fs::remove_dir_all(&tmp).await.ok();
        return Err(e);
    }
    tokio::fs::remove_dir_all(ext_dir).await?;
    if let Err(e) = tokio::fs::rename(&tmp, ext_dir).await {
        // rename 失败（跨设备等）：旧目录已删，把新副本放回去尽力收敛。
        let _ = copy_dir(source, ext_dir).await;
        tokio::fs::remove_dir_all(&tmp).await.ok();
        return Err(e.into());
    }
    Ok(())
}

/// 挂载一条 symlink，返回本次动作。槽位被占记入 `conflicts`（不覆盖），
/// 由调用方汇总拒绝。
async fn mount(
    link: &Path,
    target: &Path,
    is_dir: bool,
    conflicts: &mut Vec<String>,
) -> Result<MountStatus, PkgError> {
    if let Some(parent) = link.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    match tokio::fs::symlink_metadata(link).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match os_symlink(target, link, is_dir).await {
                Ok(()) => Ok(MountStatus::Linked),
                // 并发 install 竞争同一槽位：创建撞车则退到重判定——
                // 是我们的 → Already；是别人的 → 记冲突，绝不裸抛 EEXIST。
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    evaluate_existing(link, target, conflicts).await
                }
                Err(e) => Err(e.into()),
            }
        }
        Err(e) => Err(e.into()),
        Ok(_) => evaluate_existing(link, target, conflicts).await,
    }
}

/// 槽位已存在时的所有权判定：symlink 指向本包 → Already；其他 →
/// 记冲突（不覆盖），返回 Already 仅作占位（调用方看到冲突即整体拒绝）。
/// 竞态路径上槽位可能在判定瞬间消失（NotFound）——按 Already 处理，
/// 由调用方的冲突检查/重跑收敛兜住。
async fn evaluate_existing(
    link: &Path,
    target: &Path,
    conflicts: &mut Vec<String>,
) -> Result<MountStatus, PkgError> {
    match tokio::fs::symlink_metadata(link).await {
        Ok(md) if md.file_type().is_symlink() => {
            let cur = tokio::fs::read_link(link).await?;
            if cur == target {
                Ok(MountStatus::Already)
            } else {
                conflicts.push(format!("{} (symlink → {})", link.display(), cur.display()));
                Ok(MountStatus::Already)
            }
        }
        Ok(_) => {
            conflicts.push(format!("{} (existing entry)", link.display()));
            Ok(MountStatus::Already)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(MountStatus::Already),
        Err(e) => Err(e.into()),
    }
}

/// 扫描包内 hooks 条目：`hooks/<point>/<entry>`，返回 `(point, entry, is_dir)`。
/// 目录不存在视为无 hook。条目名排序保证报告与挂载顺序稳定。
async fn scan_hook_entries(pkg_dir: &Path) -> Vec<(String, String, bool)> {
    try_scan_hook_rels(pkg_dir)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|rel| {
            // rel = "hooks/<point>/<entry>"
            let mut parts = rel.splitn(3, '/');
            let _ = parts.next();
            let point = parts.next().unwrap_or_default().to_string();
            let entry = parts.next().unwrap_or_default().to_string();
            let is_dir = pkg_dir.join(&rel).is_dir();
            (point, entry, is_dir)
        })
        .collect()
}

async fn try_scan_hook_rels(pkg_dir: &Path) -> std::io::Result<Vec<String>> {
    let hooks_root = pkg_dir.join(HOOKS_DIR);
    let mut out = Vec::new();
    let mut points = tokio::fs::read_dir(&hooks_root).await?;
    while let Some(point) = points.next_entry().await? {
        if !point.file_type().await?.is_dir() {
            continue;
        }
        let point_name = point.file_name().to_string_lossy().into_owned();
        let mut entries = tokio::fs::read_dir(point.path()).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            out.push(format!("{HOOKS_DIR}/{point_name}/{name}"));
        }
    }
    out.sort();
    Ok(out)
}

/// 扫描包内 bin 扁平文件：返回 (文件名, 是否有执行位)。子目录跳过
/// （多文件工具是 tools/ 资源的事）并 warn。
async fn scan_bin_files(pkg_dir: &Path) -> Vec<(String, bool)> {
    try_scan_bin_rels(pkg_dir)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|rel| {
            let file = rel.trim_start_matches("bin/").to_string();
            let path = pkg_dir.join(&rel);
            let exec = is_executable(&path);
            (file, exec)
        })
        .collect()
}

async fn try_scan_bin_rels(pkg_dir: &Path) -> std::io::Result<Vec<String>> {
    let bin_root = pkg_dir.join(BIN_DIR);
    let mut out = Vec::new();
    let mut entries = match tokio::fs::read_dir(&bin_root).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if !entry.file_type().await?.is_file() {
            tracing::warn!(bin = %name, "bin entry is not a flat file; skipped (multi-file tools are a tools/ resource)");
            continue;
        }
        out.push(format!("{BIN_DIR}/{name}"));
    }
    out.sort();
    Ok(out)
}

/// 包内 snippet 文件名（`snippets/*.md`，排序）。
async fn list_snippets(pkg_dir: &Path) -> Vec<String> {
    let root = pkg_dir.join(SNIPPETS_DIR);
    let Ok(mut rd) = tokio::fs::read_dir(&root).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.to_ascii_lowercase().ends_with(".md") && !name.starts_with('.') {
            out.push(name);
        }
    }
    out.sort();
    out
}

/// 文件是否有任一执行位（unix；其他平台视为可执行）。
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|md| md.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// 建 symlink（unix 自动分辨文件/目录；windows 需显式区分）。
#[cfg(unix)]
async fn os_symlink(target: &Path, link: &Path, _is_dir: bool) -> std::io::Result<()> {
    tokio::fs::symlink(target, link).await
}

/// 建 symlink（windows：目录用 junction 语义，文件用 file symlink）。
#[cfg(windows)]
async fn os_symlink(target: &Path, link: &Path, is_dir: bool) -> std::io::Result<()> {
    if is_dir {
        tokio::fs::symlink_dir(target, link).await
    } else {
        tokio::fs::symlink_file(target, link).await
    }
}

/// 实体复制包目录（`--copy` 模式）：保留执行位。
async fn copy_dir(src: &Path, dst: &Path) -> Result<(), PkgError> {
    tokio::fs::create_dir_all(dst).await?;
    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type().await?;
        if ft.is_dir() {
            Box::pin(copy_dir(&from, &to)).await?;
        } else if ft.is_file() {
            tokio::fs::copy(&from, &to).await?;
        } else if ft.is_symlink() {
            // 包内 symlink：跟随到常规文件则拷内容（bin 常是 symlink
            // 进源码的脚本）；破损/指向非常规文件则跳过并 warn——
            // 静默丢弃会让 copy 模式与 symlink 模式行为分叉且无感知。
            match tokio::fs::metadata(&from).await {
                Ok(md) if md.is_file() => {
                    tokio::fs::copy(&from, &to).await?;
                }
                _ => {
                    tracing::warn!(path = %from.display(), "copy: skipping symlink that does not resolve to a file");
                }
            }
        }
    }
    Ok(())
}

/// 从安装记录重建本次安装挂载的相对路径集合（供 remove 与审计）。
impl ExtInstall {
    pub(crate) fn mount_paths(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .resources
            .hooks
            .iter()
            .map(|s| format!("{HOOKS_DIR}/{s}"))
            .collect();
        out.extend(self.resources.bins.iter().map(|s| format!("{BIN_DIR}/{s}")));
        out
    }
}

/// 安装报告中资源清单 → 记录结构。
pub(crate) fn resources_from_report(report: &InstallReport) -> super::store::Resources {
    super::store::Resources {
        cron: report.cron.iter().map(|c| c.name.clone()).collect(),
        hooks: report
            .hooks
            .iter()
            .map(|m| {
                m.path
                    .trim_start_matches(&format!("{HOOKS_DIR}/"))
                    .to_string()
            })
            .collect(),
        bins: report
            .bins
            .iter()
            .map(|m| {
                m.path
                    .trim_start_matches(&format!("{BIN_DIR}/"))
                    .to_string()
            })
            .collect(),
        snippets: report.snippets.clone(),
    }
}

#[allow(dead_code)]
fn _assert_send_sync() {
    fn assert<T: Send + Sync>() {}
    assert::<InstallReport>();
    assert::<RemoveReport>();
}

#[cfg(test)]
#[path = "install_test.rs"]
mod tests;
