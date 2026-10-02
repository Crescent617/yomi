//! install/remove：扩展包的物化与回收。
//!
//! 规则全文见 `docs/design/ext-packages.md` 与模块根 doc。承重墙：
//! 所有权 = symlink 文本目标相等；cron 全名 `ext:<名>:` 前缀；
//! install 纯 additive；remove 只删能证明属于自己的东西。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::installed::{InstalledExt, Resources};
use super::{cron_name, ExtError, BIN_DIR, DIR_NAME, HOOKS_DIR, SNIPPETS_DIR};
use crate::cron::{CronAction, CronSessionTemplate, CronStore};
use crate::permission::Level;

/// 一次安装的完整报告（wire 序列化给 CLI 展示，同时进安装记录）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallReport {
    pub name: String,
    pub version: String,
    /// 安装内容的内容 hash（blake3，十六进制；不含 ext.lock）：
    /// list/doctor 的本地改动侦测基准。
    pub content_hash: String,
    /// cron 收养结果：`created=false` = 已存在未动（ensure 语义）。
    pub cron: Vec<CronAdoptReport>,
    pub hooks: Vec<MountReport>,
    pub bins: Vec<MountReport>,
    /// init 钩子执行结果；未声明 init 的扩展为 None。
    #[serde(default)]
    pub init: Option<InitReport>,
    /// 包内 snippet 文件名（约定式资源，不物化）。
    pub snippets: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CronAdoptReport {
    pub name: String,
    pub status: CronAdoptStatus,
}

/// cron 收养三态：`Created` 缺才建；`Updated` 已存在但包内内容变了
/// （消息/模板/precheck 刷新到包内值）；`Untouched` 已存在且一致。
/// schedule/max_runs/expires_at 是用户部署时机，任何态都不动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronAdoptStatus {
    Created,
    Updated,
    Untouched,
}

/// init 钩子执行结果。init 是幂等 ensure（非资源），不参与回滚清单。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InitReport {
    /// 包内脚本相对路径（manifest `ext.init`）。
    pub path: String,
    /// 截断的输出尾（成功也留，便于"装完到底干了什么"的审计）。
    pub output: String,
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
/// 可否原位刷新由本函数持锁后从注册表判定：有条目 = 更新语义原位
/// 替换；无条目 = fresh 安装（槽位必须为空，被占即拒绝）。旧条目
/// 同时用于刷新后清扫旧版本遗留的失效挂载（v2 删了的 hook/bin，
/// 目录已被替换，旧 symlink 悬空且 health 看不见）。
pub async fn install(
    data_dir: &Path,
    source: &Path,
    cron_store: &Arc<dyn CronStore>,
    config_auto_approve: Level,
    // 调用方预先解析校验好的 manifest。**name 以它准**——调用方查
    // 安装记录用的必须是同一个 manifest，否则两次 parse 之间文件被
    // 换名会导致刷新拿着 A 的记录删 B 槽位的目录（TOCTOU）。
    manifest: &super::ExtManifest,
    // 来源溯源：写进注册表 ext.lock。
    provenance: &super::Provenance,
) -> Result<InstallReport, ExtError> {
    let source = source
        .canonicalize()
        .map_err(|e| ExtError::Invalid(format!("package dir {}: {e}", source.display())))?;
    if !source.is_dir() {
        return Err(ExtError::Invalid(format!(
            "package dir {} is not a directory",
            source.display()
        )));
    }
    let name = manifest.ext.name.clone();
    // 注册表是单文件整表重写：所有 install/remove 经全局锁串行
    // （各自的"原子"操作叠加起来不原子）。扩展数量级下串行无感。
    let _guard = registry_lock().lock().await;
    let ext_dir = data_dir.join(DIR_NAME).join(&name);
    // 注册表整表载入内存（严格模式：文件在但损坏 → 拒绝——整表重写
    // 会把其他扩展的条目一起抹掉），各阶段改条目后原子重写。
    let mut lockfile = super::read_lockfile_strict(data_dir).map_err(ExtError::Storage)?;
    // previous 一律锁内从注册表取（调用方在锁外预读的可能是陈旧
    // 快照，不可取）；无条目 = 槽位空或 foreign（用户手放）。
    let previous = lockfile.get(&name).cloned();
    // 清掉上次刷新崩溃遗留的隐藏临时目录（copy_refresh 的 .<名>.tmp，
    // 点开头扫描器不可见，不清就成永久泄漏）。
    let stray_tmp = data_dir.join(DIR_NAME).join(format!(".{name}.tmp"));
    if tokio::fs::symlink_metadata(&stray_tmp).await.is_ok() {
        if tokio::fs::remove_dir_all(&stray_tmp).await.is_err() {
            let _ = tokio::fs::remove_file(&stray_tmp).await;
        }
    }

    // 先对源做完整 walk（含 1MB 上限与 ext.toml 可解析性）：超限在此
    // 拒绝，不碰任何槽位——此前把这一步放在复制/挂载/cron 之后，拒
    // 绝时留下的无记录目录会被重跑当成 occupied 撞 Conflict。
    hash_package(&source)?;

    place_or_refresh(&ext_dir, &source, previous.is_some()).await?;

    // 目录已是事实而挂载/cron 还没走完：立刻落一条 provisional 条目
    // （资源清单沿用旧记录——fresh 安装为空表）。崩溃/失败发生在
    // 其后任一点：重跑拿到 previous 原位收敛；remove 精确回滚 =
    // 本条目的旧资源 ∪ 包目录扫描（兜底已建挂载）∪ cron 前缀清扫
    // （兜底已收养 job）。否则无记录目录把重跑挡成 occupied、remove
    // 按 foreign 拒绝（"任何中断可重跑收敛"的缺口就在这个 cut point）。
    let prov_resources = previous
        .as_ref()
        .map(|p| p.resources.clone())
        .unwrap_or_default();
    let prov_hash = hash_package(&ext_dir).unwrap_or_else(|e| {
        tracing::warn!(ext = %name, "provisional install hash failed: {e}");
        String::new()
    });
    if let Err(e) = persist_entry(
        data_dir,
        &mut lockfile,
        make_entry(&name, provenance, &prov_hash, &prov_resources),
    ) {
        // best-effort：最终 persist 在末尾还有一次，那边失败会整体报错。
        tracing::warn!(ext = %name, error = %e, "provisional persist failed");
    }

    // 悬空挂载清扫：目录内容已被替换，凡 symlink 精确指向本包、而新
    // 包没有声明的 hook/bin 挂载必然悬空——留着会挡别的扩展装同名
    // 槽位（phantom mount conflict），health 还看不见（hash 只扫包
    // 目录）。**扫全盘挂载树而不是旧记录名单**：两次连续中断的刷新
    // 之间，上次崩溃前建的挂载既不在旧条目（provisional 资源沿用旧
    // 记录）也不在新包目录里，按名单清扫会漏成永久悬空。指向不符
    // （用户动过）留 warn 给 remove。
    sweep_stale_mounts(data_dir, &ext_dir).await?;

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
        // 挂载冲突整体拒绝。目录与已建挂载已是事实，但不必在这里落
        // 部分记录：provisional 条目已保证重跑原位收敛，remove 靠
        // "旧资源 ∪ 包目录扫描 ∪ cron 前缀清扫"精确回滚。
        return Err(ExtError::Conflict(conflicts.join("; ")));
    }

    // init 钩子（manifest ext.init 声明才跑）：从已装目录执行，
    // 标准 yomi 子进程环境（注入 YOMI_DATA_DIR、PATH 含 <data_dir>/bin，
    // 见 run_init）。幂等是作者约定，与 cron ensure 同哲学——每次
    // install/refresh 都跑。失败整体报错：provisional 条目已保证
    // 重跑原位收敛。注意本步在注册表互斥锁内最长 120s——期间其他
    // install/remove 排队（非死锁：init 不反向取注册表锁；init 经
    // PATH 里的 yomi CLI 发 extension install/remove RPC 会自锁，由
    // 超时解开，有界停顿 120s）。
    let init = run_init(data_dir, &ext_dir, manifest).await?;

    // cron 收养（refresh 语义：缺才建，已存在则**内容随包更新**——
    // 消息/会话模板/precheck 刷新到包内值；schedule/max_runs/expires_at
    // 是用户部署时机，不动）。与 remove 的前缀清扫口径一致：ext:<名>:
    // 命名空间归扩展所有，装时不让改、卸时全删的"半保护"不再成立。
    // 消息从**已装目录**取（单一事实源 = 已装内容，与 hash/copy 同源；
    // 取货临时目录此处可能已被清理）。
    let mut cron = Vec::new();
    for entry in &manifest.cron {
        let full = cron_name(&name, &entry.name);
        let content = entry.resolve_message(&ext_dir)?;
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
        // 收养失败直接返回：provisional 条目已保证重跑原位收敛；已建
        // 的 cron job 由 remove 的前缀清扫兜底，无需部分落盘。
        let outcome =
            crate::cron::create_cron_job(cron_store, None, input.clone(), config_auto_approve)
                .await
                .map_err(ExtError::Cron)?;
        let status = if outcome.created {
            CronAdoptStatus::Created
        } else if refresh_cron_content(cron_store, &outcome.job, &input, config_auto_approve)
            .await?
        {
            CronAdoptStatus::Updated
        } else {
            CronAdoptStatus::Untouched
        };
        cron.push(CronAdoptReport { name: full, status });
    }

    let snippets = list_snippets(&ext_dir).await;
    let content_hash = hash_package(&ext_dir)?;

    // 注册表条目（hash 之后落盘：content_hash 是终值）。落盘失败
    // 整体报错：报成功却没条目 = remove 按 foreign 拒绝的孤儿。
    let resources = resources_from_report(&cron, &hooks, &bins, &snippets);
    persist_entry(
        data_dir,
        &mut lockfile,
        make_entry(&name, provenance, &content_hash, &resources),
    )
    .map_err(|e| ExtError::Storage(format!("persist ext.lock: {e}")))?;

    Ok(InstallReport {
        name,
        version: manifest.ext.version.clone(),
        content_hash,
        cron,
        hooks,
        bins,
        init,
        snippets,
    })
}

/// init 安装钩子上限：初始化脚本约定是秒级；超时说明脚本挂了或
/// 在等交互，连后裔一起收树后判失败（install 报错、可重跑）。
const INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// init 输出保留字节数（报告与错误信息共用，看尾不看头）。
const INIT_OUTPUT_KEEP: usize = 4096;

/// 执行 manifest 声明的 init 钩子（未声明返回 None）。经统一 shell
/// 探测执行（bash 优先，Windows Git Bash→pwsh→powershell→cmd，命令
/// 文本走 wrap_command），进程树由 spawn_in_new_tree 建立（超时连
/// 后裔一起收）；标准环境注入复用 inject_child_env——脚本拿到的
/// YOMI_DATA_DIR/PATH 与任何 yomi 子进程一致。
async fn run_init(
    data_dir: &Path,
    ext_dir: &Path,
    manifest: &super::ExtManifest,
) -> Result<Option<InitReport>, ExtError> {
    let Some(rel) = &manifest.ext.init else {
        return Ok(None);
    };
    let script = ext_dir.join(rel);
    #[cfg(unix)]
    if let Ok(md) = tokio::fs::metadata(&script).await {
        use std::os::unix::fs::PermissionsExt as _;
        if md.permissions().mode() & 0o111 == 0 {
            // bash 对含斜杠的词直接 execve：无执行位 = exit 126 安装
            // 失败，不是"还能跑"。
            tracing::warn!(ext = %manifest.ext.name, init = %rel, "init script lacks exec bit; install will fail with Permission denied (chmod +x)");
        }
    }
    let shell = crate::utils::shell::detect();
    // manifest 校验已限字符集（字母数字 ._/-），命令文本无注入面。
    let wrapped = shell.wrap_command(rel);
    let mut cmd = tokio::process::Command::new(&shell.path);
    cmd.args(shell.leading_args())
        .arg(wrapped.as_ref())
        .current_dir(ext_dir)
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0");
    crate::utils::env::inject_child_env(&mut cmd, Some(data_dir), None);
    // 执行引擎复用 spawn_captured（hooks/tools 同款）：stdio 接管、
    // 双管并发排空（≤64KB cap，chatty 脚本不会吃内存）、超时按树强杀。
    let cap = crate::utils::spawn::spawn_captured(&mut cmd, None, INIT_TIMEOUT, None)
        .await
        .map_err(|e| ExtError::Init(format!("run '{rel}': {e}")))?;
    if cap.timed_out {
        return Err(ExtError::Init(format!(
            "'{rel}' timed out after {}s",
            INIT_TIMEOUT.as_secs()
        )));
    }
    let tail = tail_bytes(&cap.stdout, &cap.stderr, INIT_OUTPUT_KEEP);
    if cap.exit_code != Some(0) {
        return Err(ExtError::Init(format!(
            "'{rel}' exited {}: {tail}",
            cap.exit_code
                .map_or("by signal".to_string(), |c| c.to_string())
        )));
    }
    tracing::info!(ext = %manifest.ext.name, init = %rel, "init hook ran");
    Ok(Some(InitReport {
        path: rel.clone(),
        output: tail,
    }))
}

/// stdout/stderr 合并取末尾 keep 字节；切在多字节字符中间会前进到
/// 下一个合法边界，避免报告里出现替换符。
fn tail_bytes(stdout: &[u8], stderr: &[u8], keep: usize) -> String {
    let mut buf = stdout.to_vec();
    if !stderr.is_empty() {
        if !buf.is_empty() {
            buf.push(b'\n');
        }
        buf.extend_from_slice(stderr);
    }
    let mut start = buf.len().saturating_sub(keep);
    while start < buf.len() && (buf[start] & 0xC0) == 0x80 {
        start += 1;
    }
    String::from_utf8_lossy(&buf[start..]).into_owned()
}

/// 已存在 ext cron job 的内容对账：包内派生字段（消息文本、会话
/// 工作目录、precheck）不一致则 update 到包内值并返回 true；一致
/// 不碰 store 返回 false。权限等级是机器派生值，不参与对账（更新
/// 时按当前 config 重算，与 create_cron_job 创建路径同规则）。
async fn refresh_cron_content(
    store: &Arc<dyn CronStore>,
    existing: &crate::cron::CronJob,
    input: &crate::cron::CreateCronJobInput,
    config_auto_approve: Level,
) -> Result<bool, ExtError> {
    let new_precheck = input.precheck.clone().filter(|s| !s.trim().is_empty());
    let same = match (&existing.action, &input.action) {
        (
            crate::cron::CronAction::SendMessage {
                content: old_content,
                session_template: old_tpl,
                ..
            },
            crate::cron::CronAction::SendMessage {
                content,
                session_template,
                ..
            },
        ) => {
            let old_dir = old_tpl.as_ref().and_then(|t| t.working_dir.clone());
            let new_dir = session_template
                .as_ref()
                .and_then(|t| t.working_dir.clone());
            old_content == content && old_dir == new_dir && existing.precheck == new_precheck
        }
        // input.action 由 install 恒为 SendMessage；落在 `_` = 用户用
        // yomi cron update 把扩展 job 改成 shell——按"命名空间归扩展
        // 所有"刷回包内值，与 remove 前缀清扫口径一致，不是漏判。
        _ => false,
    };
    if same {
        return Ok(false);
    }
    // precheck 的 update 语义：Some("") = 清除——作者删了闸门，刷新
    // 跟着清。
    let action = match input.action.clone() {
        crate::cron::CronAction::SendMessage {
            session_id: None,
            content,
            session_template,
        } => {
            // 模板物化对齐 create_cron_job 路径：无 work_dir 也落
            // Some(默认模板 + 重算等级)，不让 store 形状在刷新后退化。
            let mut tpl = session_template.unwrap_or(crate::cron::CronSessionTemplate {
                working_dir: None,
                project_id: None,
                auto_approve_level: None,
            });
            tpl.auto_approve_level = Some(
                config_auto_approve
                    .max(crate::permission::Level::Caution)
                    .as_str()
                    .to_string(),
            );
            crate::cron::CronAction::SendMessage {
                session_id: None,
                content,
                session_template: Some(tpl),
            }
        }
        other => other,
    };
    store
        .update(
            &existing.id,
            &crate::cron::UpdateCronJobInput {
                name: None,
                schedule: None,
                action: Some(action),
                status: None,
                max_runs: None,
                expires_at: None,
                precheck: Some(input.precheck.clone().unwrap_or_default()),
                next_run_at: None,
            },
        )
        .await
        .map_err(ExtError::Cron)?;
    tracing::info!(cron = %existing.name, "extension refresh updated cron content");
    Ok(true)
}

/// 包内容 hash：blake3，按相对路径排序后逐文件喂（路径+内容），对
/// 复制时刻的包内容做指纹。单文件上限 1MB——包是"约定 + 小脚本"的
/// 载体，藏超大文件按恶意/损坏处理，install 直接拒。
const HASH_FILE_MAX_BYTES: u64 = 1024 * 1024;
/// 包总字节上限：防"一堆 1MB 文件拼出大盘"（逐文件闸管不住总量）。
const PACKAGE_TOTAL_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// 包文件数上限：防 walk 炸弹（每文件都合法，但数量把 hash/copy 拖死）。
const PACKAGE_FILE_MAX: usize = 10_000;

/// 包内容 hash（blake3）：install 落指纹与 list/doctor 的本地改动侦测
/// 共用同一算法。
pub fn package_hash(dir: &Path) -> Result<String, ExtError> {
    hash_package(dir)
}

fn hash_package(dir: &Path) -> Result<String, ExtError> {
    let mut hasher = blake3::Hasher::new();
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(dir, &mut files)?;
    files.sort();
    let mut total: u64 = 0;
    for file in files {
        let rel = file.strip_prefix(dir).unwrap_or(&file);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(&[0]);
        let mut buf = read_bounded(&file, rel)?;
        total += buf.len() as u64;
        if total > PACKAGE_TOTAL_MAX_BYTES {
            return Err(ExtError::Oversized(format!(
                "package total exceeds {PACKAGE_TOTAL_MAX_BYTES} bytes"
            )));
        }
        hasher.update(&std::mem::take(&mut buf));
        hasher.update(&[0xFF]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// 读取一个待 hash 文件的内容：fstat 预检尺寸 + `take` 封顶读取——
/// 元数据检查与读之间有竞态窗（装到一半文件被换成超大文件），take
/// 保证内存占用有界。单文件上限 1MB——包是"约定 + 小脚本"的载体，
/// 藏超大文件按恶意/损坏处理，install 直接拒。
fn read_bounded(file: &Path, rel: &Path) -> Result<Vec<u8>, ExtError> {
    let f = std::fs::File::open(file)?;
    if f.metadata()?.len() > HASH_FILE_MAX_BYTES {
        return Err(ExtError::Oversized(format!(
            "package file {} exceeds {HASH_FILE_MAX_BYTES} bytes",
            rel.display()
        )));
    }
    let mut capped = std::io::Read::take(f, HASH_FILE_MAX_BYTES + 1);
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut capped, &mut raw)?;
    if raw.len() as u64 > HASH_FILE_MAX_BYTES {
        return Err(ExtError::Oversized(format!(
            "package file {} exceeds {HASH_FILE_MAX_BYTES} bytes",
            rel.display()
        )));
    }
    Ok(raw)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ExtError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            // `.git` 是 VCS 元数据不是包内容：git 源 clone 到临时目录后
            // 直接喂 install（默认玩法），pack 文件动辄超 1MB 会被误判成
            // "包文件超限"拒装，复制进 extensions/ 也是纯膨胀。hash 与
            // copy_dir 同口径跳过，一致性不受破坏。
            if entry.file_name() == ".git" {
                continue;
            }
            collect_files(&path, out)?;
        } else if ft.is_file() {
            if out.len() >= PACKAGE_FILE_MAX {
                return Err(ExtError::Oversized(format!(
                    "package exceeds {PACKAGE_FILE_MAX} files"
                )));
            }
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
) -> Result<RemoveReport, ExtError> {
    // 名字与 install 同一规则校验：`../` 之类的穿越在此被硬拒（否则
    // join 后 remove_dir_all 会删到数据目录外）。
    if !super::manifest::valid_ext_name(name) {
        return Err(ExtError::Invalid(format!(
            "extension name '{name}' invalid: letter first, [a-z0-9-] only, ≤32 chars"
        )));
    }
    // 与 install 同一把注册表全局锁：单文件整表重写下必须互斥。
    let _guard = registry_lock().lock().await;
    remove_inner(data_dir, name, cron_store, installed).await
}

async fn remove_inner(
    data_dir: &Path,
    name: &str,
    cron_store: &Arc<dyn CronStore>,
    installed: Option<&InstalledExt>,
) -> Result<RemoveReport, ExtError> {
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

    // 挂载名单：已装清单（ext.lock）优先；包目录还在则并集（清单
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
                    tracing::warn!(ext = %name, link = %link.display(), target = %cur.display(), "mount slot repointed; leaving it");
                    mounts_left.push(rel);
                }
            }
            Ok(_) => {
                tracing::warn!(ext = %name, link = %link.display(), "mount slot replaced with a non-symlink; leaving it");
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
        // 实体目录：只有注册表条目证明是我们装的才删；用户把自己的
        // 目录放进槽位时留下并 warn（设计：全程只动能证明属于自己
        // 的东西）。
        Ok(_) if installed.is_some_and(|i| i.meta.is_some()) => {
            tokio::fs::remove_dir_all(&ext_dir).await?;
            true
        }
        Ok(_) => {
            tracing::warn!(dir = %ext_dir.display(), "extensions slot is a non-symlink dir not owned by this install record; leaving it");
            false
        }
    };
    if ext_dir_removed {
        // 注册表条目随删除移除（ghost 条目只会误导 list/health）。
        // 宽松读即可：损坏 → 空表 → remove(name) 为 false 不写盘，
        // 不存在抹掉其他条目的窗口。
        let mut lf = super::read_lockfile(data_dir);
        if lf.remove(name) {
            if let Err(e) = super::write_lockfile(data_dir, &lf) {
                tracing::warn!(error = %e, "failed to persist ext.lock entry removal");
            }
        }
    }

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
) -> Result<(), ExtError> {
    match tokio::fs::symlink_metadata(ext_dir).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = ext_dir.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            // 与刷新同款 tmp+swap：kill -9 落在复制中途只留 .tmp 孤儿
            // （install 起点清理），不留半个无记录的 ext_dir——直写
            // 的话重跑撞 occupied、remove 按 foreign 拒绝，不可收敛。
            copy_refresh(source, ext_dir).await
        }
        Err(e) => Err(e.into()),
        Ok(_) if allow_replace => copy_refresh(source, ext_dir).await,
        Ok(_) => Err(ExtError::Conflict(format!(
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
async fn copy_refresh(source: &Path, ext_dir: &Path) -> Result<(), ExtError> {
    let tmp = ext_dir.parent().unwrap_or(ext_dir).join(format!(
        ".{}.tmp",
        ext_dir.file_name().unwrap_or_default().to_string_lossy()
    ));
    tokio::fs::remove_dir_all(&tmp).await.ok();
    if let Err(e) = copy_dir(source, &tmp).await {
        tokio::fs::remove_dir_all(&tmp).await.ok();
        return Err(e);
    }
    // fresh 路径目标本不存在；refresh 路径删旧目录。.ok() 两者兼容。
    tokio::fs::remove_dir_all(ext_dir).await.ok();
    if let Err(e) = tokio::fs::rename(&tmp, ext_dir).await {
        // rename 失败（跨设备等）：旧目录已删，把新副本放回去尽力收敛。
        let _ = copy_dir(source, ext_dir).await;
        tokio::fs::remove_dir_all(&tmp).await.ok();
        return Err(e.into());
    }
    Ok(())
}

/// 注册表全局串行锁：单文件整表重写下，install/remove 全操作互斥。
fn registry_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// 组装一条注册表条目。
fn make_entry(
    name: &str,
    provenance: &super::Provenance,
    content_hash: &str,
    resources: &Resources,
) -> super::LockEntry {
    super::LockEntry {
        name: name.to_string(),
        source: provenance.source.clone(),
        rev: provenance.rev.clone(),
        content_hash: content_hash.to_string(),
        resources: resources.clone(),
        installed_at: chrono::Utc::now(),
    }
}

/// 把条目 upsert 进注册表并原子落盘。调用方决定成败：provisional/
/// 收养失败仅 warn（重跑/后续最终 persist 会再收敛）；最终 persist
/// 失败必须让整个 install 报错——报成功却没条目，remove 会按
/// foreign 拒绝，装了个不可收敛的孤儿。
fn persist_entry(
    data_dir: &Path,
    lockfile: &mut super::ExtLockfile,
    entry: super::LockEntry,
) -> Result<(), String> {
    lockfile.upsert(entry);
    super::write_lockfile(data_dir, lockfile)
}

/// 挂载一条 symlink，返回本次动作。槽位被占记入 `conflicts`（不覆盖），
/// 由调用方汇总拒绝。
async fn mount(
    link: &Path,
    target: &Path,
    is_dir: bool,
    conflicts: &mut Vec<String>,
) -> Result<MountStatus, ExtError> {
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
) -> Result<MountStatus, ExtError> {
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
            tracing::warn!(ext = %pkg_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), bin = %name, "bin entry is not a flat file; skipped (multi-file tools are a tools/ resource)");
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

/// 实体复制包目录：保留执行位。逐文件有界（fstat 预检 + take 封顶
/// 读取，与 hash 的 read_bounded 同口径——源在 hash 后被换成超大文件
/// 的竞态窗在此闭合），并累计总量/文件数，超限即拒不落地。
async fn copy_dir(src: &Path, dst: &Path) -> Result<(), ExtError> {
    copy_dir_limited(src, dst, &mut 0, &mut 0).await
}

/// 有界读一个包文件并写到目标，保留权限位（exec bit 是 bin 的
/// 生效条件）。fstat 预检 + take 封顶读取，与 hash 的 read_bounded
/// 同口径。
async fn copy_file_bounded(from: &Path, to: &Path) -> Result<Vec<u8>, ExtError> {
    let data = read_bounded(from, from)?;
    tokio::fs::write(to, &data).await?;
    // write 新建文件用默认权限，执行位要显式带回。
    if let Ok(md) = std::fs::metadata(from) {
        let _ = tokio::fs::set_permissions(to, md.permissions()).await;
    }
    Ok(data)
}

async fn copy_dir_limited(
    src: &Path,
    dst: &Path,
    total: &mut u64,
    count: &mut usize,
) -> Result<(), ExtError> {
    tokio::fs::create_dir_all(dst).await?;
    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type().await?;
        if ft.is_dir() {
            // `.git` 不复制（VCS 元数据非包内容；与 collect_files 的
            // hash 跳过同口径）。
            if entry.file_name() == ".git" {
                continue;
            }
            Box::pin(copy_dir_limited(&from, &to, total, count)).await?;
        } else if ft.is_file() {
            *count += 1;
            if *count > PACKAGE_FILE_MAX {
                return Err(ExtError::Oversized(format!(
                    "package exceeds {PACKAGE_FILE_MAX} files"
                )));
            }
            let data = copy_file_bounded(&from, &to).await?;
            *total += data.len() as u64;
            if *total > PACKAGE_TOTAL_MAX_BYTES {
                return Err(ExtError::Oversized(format!(
                    "package total exceeds {PACKAGE_TOTAL_MAX_BYTES} bytes"
                )));
            }
        } else if ft.is_symlink() {
            // 包内 symlink：跟随到常规文件则拷内容（bin 常是 symlink
            // 进源码的脚本）；破损/指向非常规文件则跳过并 warn——
            // 静默丢弃会让 copy 模式与 symlink 模式行为分叉且无感知。
            match tokio::fs::metadata(&from).await {
                Ok(md) if md.is_file() => {
                    *count += 1;
                    if *count > PACKAGE_FILE_MAX {
                        return Err(ExtError::Oversized(format!(
                            "package exceeds {PACKAGE_FILE_MAX} files"
                        )));
                    }
                    let data = copy_file_bounded(&from, &to).await?;
                    *total += data.len() as u64;
                    if *total > PACKAGE_TOTAL_MAX_BYTES {
                        return Err(ExtError::Oversized(format!(
                            "package total exceeds {PACKAGE_TOTAL_MAX_BYTES} bytes"
                        )));
                    }
                }
                _ => {
                    tracing::warn!(path = %from.display(), "copy: skipping symlink that does not resolve to a file");
                }
            }
        }
    }
    Ok(())
}

/// 悬空挂载清扫（见 install 调用处注释：扫全盘挂载树，不信名单）。
/// hooks/ 递归、bin/ 浅层，统一按目录走一遍：symlink 且文本目标
/// 等于本包对应路径（所有权规则）、rel 又不在新包声明集合里 → 摘。
async fn sweep_stale_mounts(data_dir: &Path, ext_dir: &Path) -> Result<(), ExtError> {
    let mut keep: std::collections::HashSet<String> = try_scan_hook_rels(ext_dir)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    keep.extend(try_scan_bin_rels(ext_dir).await.unwrap_or_default());
    for root in [data_dir.join(HOOKS_DIR), data_dir.join(BIN_DIR)] {
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let mut rd = match tokio::fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            while let Some(ent) = rd.next_entry().await? {
                let path = ent.path();
                let ft = ent.file_type().await?;
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !ft.is_symlink() {
                    continue;
                }
                let rel = path
                    .strip_prefix(data_dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if keep.contains(&rel) {
                    continue;
                }
                let cur = tokio::fs::read_link(&path).await?;
                if cur == ext_dir.join(&rel) {
                    tokio::fs::remove_file(&path).await?;
                    tracing::info!(mount = %rel, "swept stale mount dropped by the new package version");
                } else {
                    tracing::warn!(mount = %path.display(), target = %cur.display(), "mount slot repointed; leaving it");
                }
            }
        }
    }
    Ok(())
}

/// 资源清单组装（写入 ext.lock；remove 按它精确回滚）。
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
