//! install/remove：扩展包的物化与回收。
//!
//! 规则全文见 `docs/design/ext-packages.md` 与模块根 doc。承重墙：
//! 所有权 = symlink 文本目标相等；cron 全名 `ext:<名>:` 前缀；
//! install 纯 additive；remove 只删能证明属于自己的东西。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::installed::{InstalledExt, Resources};
use super::{cron_name, PkgError, BIN_DIR, DIR_NAME, HOOKS_DIR, MANIFEST_FILE, SNIPPETS_DIR};
use crate::cron::{CronAction, CronSessionTemplate, CronStore};
use crate::permission::Level;

/// 一次安装的完整报告（wire 序列化给 CLI 展示，同时进安装记录）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallReport {
    pub name: String,
    pub version: String,
    /// 安装内容的内容 hash（blake3，十六进制；不含 [install] 段）：
    /// list/doctor 的本地改动侦测基准。
    pub content_hash: String,
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
    /// `extensions/<名>` 是否已删除（清单缺失/foreign 时留下不删）。
    pub ext_dir_removed: bool,
}

/// 收编 + 挂载 + 收养 cron。统一 copy：把 `source`（取货后的包根目录，
/// 调用方持有临时目录句柄）实体复制进 extensions/<名>，重装即重新
/// 取货 + 原位替换（更新语义）。幂等可重放。
///
/// `allow_replace`：槽位已有实体目录且调用方确认是我们上次装的
/// （有安装记录）才允许原位刷新；否则拒绝（绝不覆盖用户目录）。
pub async fn install(
    data_dir: &Path,
    source: &Path,
    cron_store: &Arc<dyn CronStore>,
    config_auto_approve: Level,
    // 调用方预先解析校验好的 manifest。**name 以它准**——调用方查
    // 安装记录用的必须是同一个 manifest，否则两次 parse 之间文件被
    // 换名会导致刷新拿着 A 的记录删 B 槽位的目录（TOCTOU）。
    manifest: &super::ExtManifest,
    allow_replace: bool,
    // 来源溯源：写进已装目录 ext.toml 的 [install] 段。
    provenance: &super::Provenance,
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
    // 同名包串行：并发的两次 install/remove 会在 copy_refresh 的临时
    // 目录、ext.toml 重写上互踩（各自的"原子"操作叠加起来不原子）。
    let _lock = pkg_lock(&name);
    let _guard = _lock.lock().await;
    let ext_dir = data_dir.join(DIR_NAME).join(&name);

    // 先对源做完整 walk（含 1MB 上限与 ext.toml 可解析性）：超限在此
    // 拒绝，不碰任何槽位——此前把这一步放在复制/挂载/cron 之后，拒
    // 绝时留下的无段目录会被重跑当成 occupied 撞 Conflict。
    hash_package(&source)?;

    place_or_refresh(&ext_dir, &source, allow_replace).await?;
    // 包内自带的 [install] 表在复制后剥掉：它要么是伪装的归属证明
    // （remove 会因此误删用户目录），要么与随后写入的真段撞重复表、
    // 整篇 TOML 解析失败。剥离后 ext.toml 内容进 hash，两侧规则一致。
    strip_install_section(&ext_dir)?;

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
        // 挂载冲突整体拒绝，但目录与已建挂载已是事实。落一份部分资源
        // 的 [install] 段（best-effort）：重跑 install 据此拿到
        // allow_replace 原位刷新收敛，remove 也能精确回滚已建部分——
        // 否则无段目录会把重跑挡成 occupied Conflict，"re-run to
        // converge" 成空话。
        let partial_hash = hash_package(&ext_dir).unwrap_or_else(|e| {
            // 仅 IO 竞态可触发；空串 hash 让 health 保持 modified 直到
            // 重跑收敛，warn 留痕。
            tracing::warn!(ext = %name, "partial install hash failed: {e}");
            String::new()
        });
        let partial = resources_from_report(&[], &hooks, &bins, &[]);
        if let Err(e) = super::write_install_meta(&ext_dir, &partial_hash, provenance, &partial)
            .map_err(PkgError::Invalid)
        {
            tracing::warn!(ext = %name, "failed to record partial install: {e}");
        }
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
    let content_hash = hash_package(&ext_dir)?;

    // [install] 段（hash 之后写：段是我们的、每次重装重写，不入 hash）。
    let resources = resources_from_report(&cron, &hooks, &bins, &snippets);
    super::write_install_meta(&ext_dir, &content_hash, provenance, &resources)
        .map_err(PkgError::Invalid)?;

    Ok(InstallReport {
        name,
        version: manifest.ext.version.clone(),
        content_hash,
        cron,
        hooks,
        bins,
        snippets,
    })
}

/// 包内容 hash：blake3，按相对路径排序后逐文件喂（路径+内容），对
/// 复制时刻的包内容做指纹。单文件上限 1MB——包是"约定 + 小脚本"的
/// 载体，藏超大文件按恶意/损坏处理，install 直接拒。
const HASH_FILE_MAX_BYTES: u64 = 1024 * 1024;

/// 包内容 hash（blake3）：install 落指纹与 list/doctor 的本地改动侦测
/// 共用同一算法。
pub fn package_hash(dir: &Path) -> Result<String, PkgError> {
    hash_package(dir)
}

fn hash_package(dir: &Path) -> Result<String, PkgError> {
    let mut hasher = blake3::Hasher::new();
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(dir, &mut files)?;
    files.sort();
    for file in files {
        let rel = file.strip_prefix(dir).unwrap_or(&file);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        let mut buf = read_hash_input(dir, &file)?;
        if buf.len() as u64 > HASH_FILE_MAX_BYTES {
            return Err(PkgError::Invalid(format!(
                "package file {} exceeds {HASH_FILE_MAX_BYTES} bytes",
                rel.display()
            )));
        }
        hasher.update(&std::mem::take(&mut buf));
        hasher.update(&[0xFF]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 读取一个待 hash 文件的内容。根 ext.toml 特殊处理：剥掉 `[install]`
/// 表再序列化后 hash——该段是我们的、每次重装重写（见
/// `write_install_meta`），两边用同一规则才能比对出本地改动。其余
/// 文件原样读。单文件上限 1MB——包是"约定 + 小脚本"的载体，藏超大
/// 文件按恶意/损坏处理，install 直接拒。
fn read_hash_input(dir: &Path, file: &Path) -> Result<Vec<u8>, PkgError> {
    let rel = file.strip_prefix(dir).unwrap_or(file);
    let f = std::fs::File::open(file)?;
    // fstat 先看尺寸；读取用 take 封顶——元数据检查与读之间有竞态窗
    // （装到一半文件被换成超大文件），take 保证内存占用有界。
    if f.metadata()?.len() > HASH_FILE_MAX_BYTES {
        return Err(PkgError::Invalid(format!(
            "package file {} exceeds {HASH_FILE_MAX_BYTES} bytes",
            rel.display()
        )));
    }
    let mut capped = std::io::Read::take(f, HASH_FILE_MAX_BYTES + 1);
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut capped, &mut raw)?;
    if raw.len() as u64 > HASH_FILE_MAX_BYTES {
        return Err(PkgError::Invalid(format!(
            "package file {} exceeds {HASH_FILE_MAX_BYTES} bytes",
            rel.display()
        )));
    }
    // 大小写不敏感比较文件名：macOS/Windows 文件系统对大小写不敏感，
    // 仓里叫 EXT.TOML 的包 parse_manifest 能读进，hash 侧也必须剥段，
    // 否则装完即永久 modified。实现按文件名匹配（不限根目录）——比
    // 磁盘剥段（只剥根）宽，但两侧 hash 同一函数，比对仍一致。
    if rel
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("ext.toml"))
    {
        if let Ok(text) = std::str::from_utf8(&raw) {
            if let Ok(mut table) = text.parse::<toml::Table>() {
                table.remove("install");
                if let Ok(text) = toml::to_string_pretty(&table) {
                    return Ok(text.into_bytes());
                }
            }
        }
        // 非 UTF-8 / 解析失败：回落原样 hash（parse_manifest 在 install
        // 路径已门禁，list 重算走不到这）。
    }
    Ok(raw)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), PkgError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_files(&path, out)?;
        } else if ft.is_file() {
            out.push(path);
        } else if ft.is_symlink() {
            // symlink：跟随——解析到常规文件则按其内容 hash（源侧 bin
            // 常以 symlink 进仓，copy_dir 会把内容物化，两侧必须一致；
            // 也堵住"装后把文件换成 symlink 逃过改动侦测"的口子）。破损
            // 或指向非常规文件：跳过（copy_dir 同样丢弃，行为一致）。
            if let Ok(md) = std::fs::metadata(&path) {
                if md.is_file() {
                    out.push(path);
                }
            }
        }
    }
    Ok(())
}

/// 卸载：cron 前缀清扫 → 摘挂载（指向判定）→ 删 extensions/<名>。
/// 记录缺失时退化为扫包目录 + cron 前缀（正确性不依赖记录）。
pub async fn remove(
    data_dir: &Path,
    name: &str,
    cron_store: &Arc<dyn CronStore>,
    installed: Option<&InstalledExt>,
) -> Result<RemoveReport, PkgError> {
    // 名字与 install 同一规则校验：`../` 之类的穿越在此被硬拒（否则
    // join 后 remove_dir_all 会删到数据目录外）。
    if !super::manifest::valid_ext_name(name) {
        return Err(PkgError::Invalid(format!(
            "extension name '{name}' invalid: letter first, [a-z0-9-] only, ≤32 chars"
        )));
    }
    // 与 install 同一把按名锁：remove 与并发 install 互踩（安装写段时
    // 卸载删目录）比两个 install 互踩更容易发生。
    let _lock = pkg_lock(name);
    let _guard = _lock.lock().await;
    remove_inner(data_dir, name, cron_store, installed).await
}

async fn remove_inner(
    data_dir: &Path,
    name: &str,
    cron_store: &Arc<dyn CronStore>,
    installed: Option<&InstalledExt>,
) -> Result<RemoveReport, PkgError> {
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

    // 挂载名单：已装清单（[install] 段）优先；包目录还在则并集（清单
    // 缺失的退化路径）。scan 失败静默用名单即可。
    let mut mount_rels: Vec<String> = installed.map(|i| i.mount_paths()).unwrap_or_default();
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
        // 实体目录：只有 [install] 段证明是我们装的才删；用户把自己的
        // 目录放进槽位时留下并 warn（设计：全程只动能证明属于自己的东西）。
        Ok(_) if installed.is_some_and(|i| i.meta.is_some()) => {
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
    })
}

/// `extensions/<名>` 槽位收编：空 → 实体复制；实体目录 + `allow_replace`
/// → 原位刷新（`copy_refresh` 原子替换）；其他一律拒绝（绝不覆盖）。
async fn place_or_refresh(
    ext_dir: &Path,
    source: &Path,
    allow_replace: bool,
) -> Result<(), PkgError> {
    match tokio::fs::symlink_metadata(ext_dir).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = ext_dir.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            copy_dir(source, ext_dir).await
        }
        Err(e) => Err(e.into()),
        Ok(_) if allow_replace => copy_refresh(source, ext_dir).await,
        Ok(_) => Err(PkgError::Conflict(format!(
            "extensions slot {} is occupied by something that is not this install (remove it first)",
            ext_dir.display()
        ))),
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

/// 按扩展名取进程内串行锁。并发的 install/remove 各自的原子操作
/// （`copy_refresh` 的临时目录 + swap、ext.toml 重写、目录删除）叠加
/// 起来不原子，同名包必须排队。锁表常驻（名字有界=装过的包数）。
fn pkg_lock(name: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    LOCKS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .expect("pkg lock map poisoned")
        .entry(name.to_string())
        .or_default()
        .clone()
}

/// 剥掉已装目录 ext.toml 里包内自带的 `[install]` 表（复制后、hash
/// 前调用）：防伪造归属证明（remove 据此误删用户目录）与重复表损坏
/// TOML。ext.toml 已在 manifest 校验阶段验证可解析，这里失败只可能是
/// IO 竞态——升级为 Err 让 install 拒绝（不留状态分叉）。
fn strip_install_section(ext_dir: &Path) -> Result<(), PkgError> {
    let path = ext_dir.join(MANIFEST_FILE);
    let raw = std::fs::read_to_string(&path)?;
    let mut table = raw
        .parse::<toml::Table>()
        .map_err(|e| PkgError::Invalid(format!("re-parse {}: {e}", path.display())))?;
    if table.remove("install").is_some() {
        let text = toml::to_string_pretty(&table)
            .map_err(|e| PkgError::Invalid(format!("re-serialize {}: {e}", path.display())))?;
        std::fs::write(&path, text)?;
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

/// 实体复制包目录：保留执行位。
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

/// 资源清单组装（写入 [install] 段；remove 按它精确回滚）。
fn resources_from_report(
    cron: &[CronAdoptReport],
    hooks: &[MountReport],
    bins: &[MountReport],
    snippets: &[String],
) -> Resources {
    Resources {
        cron: cron.iter().map(|c| c.name.clone()).collect(),
        hooks: hooks
            .iter()
            .map(|m| {
                m.path
                    .trim_start_matches(&format!("{HOOKS_DIR}/"))
                    .to_string()
            })
            .collect(),
        bins: bins
            .iter()
            .map(|m| {
                m.path
                    .trim_start_matches(&format!("{BIN_DIR}/"))
                    .to_string()
            })
            .collect(),
        snippets: snippets.to_vec(),
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
