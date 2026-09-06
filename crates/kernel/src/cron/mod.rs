pub mod scheduler;
pub mod store;
#[cfg(test)]
mod tests;
pub mod types;
pub mod worker;

use async_trait::async_trait;
pub use scheduler::CronScheduler;
use std::sync::Arc;
pub use store::{CronStore, SqliteCronStore};
pub use types::{
    CreateCronJobInput, CreateCronJobOutcome, CronAction, CronError, CronJob, CronJobId,
    CronJobStatus, CronSchedule, CronSessionTemplate, UpdateCronJobInput, NEVER_EXPIRES,
    UNLIMITED_MAX_RUNS,
};
pub use worker::CronWorker;

/// 执行 cron action 的接口。`CronWorker` 只依赖此 trait，不依赖 `Kernel`。
///
/// 这确保 cron 子系统与上层协调器的解耦：
/// - `Kernel` 负责 session 管理、消息发送、Shell 执行等
/// - `CronWorker` 只负责调度、超时、结果记录
#[async_trait]
pub trait CronExecutor: Send + Sync {
    async fn execute_cron_action(&self, job: &CronJob) -> Result<CronActionOutcome, CronError>;
}

/// Exit code a cron shell job uses to retire itself: the scheduler marks the
/// job `Completed` instead of scheduling the next run. Picked clear of
/// sysexits (64–78), common failures (1/2), and signal deaths (≥128).
pub const SHELL_COMPLETE_EXIT_CODE: i32 = 42;

/// 单次 cron action 的执行结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CronActionOutcome {
    /// 正常成功——继续调度。
    #[default]
    Done,
    /// Shell job 以 `SHELL_COMPLETE_EXIT_CODE` 退出——任务自我完成，不再调度。
    SelfComplete,
    /// precheck 闸门关闭——本次触发被跳过：不记录执行（`run_count` 不增、
    /// `last_error` 不动），按 schedule 正常等待下次。
    Skipped,
}

/// precheck 闸门命令的固定超时（不设 knob；与 hook 30s 同为固定值的哲学）。
pub const PRECHECK_TIMEOUT_SECS: u64 = 60;

/// precheck stdout 追加进消息体的最大字节数。
pub const MAX_SENSOR_STDOUT: usize = 2048;

/// precheck 闸门的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrecheckOutcome {
    /// exit 0——放行；附带 stdout（trim 后，可能为空）。
    Fire(String),
    /// 非 0 退出 / 超时 / 无法执行——fail closed，本次不触发。
    Skip,
}

/// 执行 precheck 闸门命令（与 cron shell job 同款加固环境）。
///
/// 契约：exit 0 = 放行（[`PrecheckOutcome::Fire`]，stdout 供调用方注入
/// 消息体，agent 不必重跑检查）；其他任何情况 = 静默跳过
/// （[`PrecheckOutcome::Skip`]）。fail closed：传感器故障时不唤醒模型，
/// 只留 tracing 日志。
pub async fn run_precheck(
    command: &str,
    working_dir: Option<&str>,
    data_dir: &std::path::Path,
) -> PrecheckOutcome {
    run_precheck_with_timeout(
        command,
        working_dir,
        data_dir,
        std::time::Duration::from_secs(PRECHECK_TIMEOUT_SECS),
    )
    .await
}

/// [`run_precheck`] 的可注入超时版本（测试用短超时覆盖超时分支）。
async fn run_precheck_with_timeout(
    command: &str,
    working_dir: Option<&str>,
    data_dir: &std::path::Path,
    timeout: std::time::Duration,
) -> PrecheckOutcome {
    let mut cmd = shell_command(command, working_dir, data_dir);
    let result = tokio::time::timeout(timeout, cmd.output()).await;
    match result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            PrecheckOutcome::Fire(stdout)
        }
        Ok(Ok(output)) => {
            tracing::info!(
                "cron precheck gate closed (exit {:?}): {}",
                output.status.code(),
                command
            );
            PrecheckOutcome::Skip
        }
        Ok(Err(e)) => {
            tracing::warn!("cron precheck failed to execute ({e}): {command}");
            PrecheckOutcome::Skip
        }
        Err(_) => {
            tracing::warn!(
                "cron precheck timed out after {}s: {command}",
                timeout.as_secs()
            );
            PrecheckOutcome::Skip
        }
    }
}

/// 把 precheck stdout 作为传感器读数追加进 `send_message` 的消息体
///（截断到 [`MAX_SENSOR_STDOUT`]）。围栏用四反引号：读数内含三反引号也不会
/// 顶破代码块。读数中的 `{{date}}` 等模板字面量先转义为空格变体——
/// 调用方随后会对整段消息做模板替换，读数内容必须原样到达模型。
pub fn append_sensor_output(content: &str, stdout: &str) -> String {
    let truncated =
        crate::utils::strs::truncate_with_suffix(stdout, MAX_SENSOR_STDOUT, "... [truncated]");
    // 与 `render_template` 的替换表保持同步（仅这三个字面量会被替换）。
    let escaped = truncated
        .replace("{{timestamp}}", "{{ timestamp }}")
        .replace("{{date}}", "{{ date }}")
        .replace("{{time}}", "{{ time }}");
    format!("{content}\n\n---\nPrecheck output:\n````\n{escaped}\n````")
}

/// 捕获 per-run session 模板：跟随调用方 session 的 `working_dir` 与
/// `project_id`（`model_key` 不继承，保持默认模型）；权限以全局 config
/// 为基线、下限 caution——cron session 无人值守，safe 阈值下 caution 级
/// 工具调用永远等不到批准，任务会卡死。
///
/// 在创建 job（`session_id` 缺省）或更新 job（显式解绑）时调用。
pub fn capture_session_template(
    follow: Option<&crate::storage::SessionInfo>,
    config_auto_approve: crate::permission::Level,
) -> CronSessionTemplate {
    CronSessionTemplate {
        working_dir: follow.and_then(|i| i.working_dir.clone()),
        project_id: follow.and_then(|i| i.project_id.clone()),
        auto_approve_level: Some(
            config_auto_approve
                .max(crate::permission::Level::Caution)
                .as_str()
                .to_string(),
        ),
    }
}

/// 为 `send_message` 的一次运行新建独立 session（stateless per run）。
///
/// 模板缺省时按最小可用配置创建（权限 caution、无 cwd/project 继承）——
/// 只可能出现在旧数据或外部写入的 job 上。session 标题取
/// 「job 名 · 本地时间」便于在会话列表区分各次运行；会话运行后保留（keep）。
pub async fn spawn_run_session(
    session_store: &Arc<dyn crate::storage::SessionStore>,
    template: Option<&CronSessionTemplate>,
    job_name: &str,
) -> Result<crate::types::SessionId, CronError> {
    let id = crate::types::SessionId::new();
    // 等级在建无人值守会话的唯一落点强制下限 caution：模板缺省、旧数据
    // 或外部写入的非法/过低等级一律被钳到 caution 或以上。
    let auto_approve_level = template
        .and_then(|t| t.auto_approve_level.as_deref())
        .and_then(|s| s.parse::<crate::permission::Level>().ok())
        .unwrap_or(crate::permission::Level::Caution)
        .max(crate::permission::Level::Caution)
        .as_str()
        .to_string();
    session_store
        .create(crate::storage::NewSession {
            project_id: template.and_then(|t| t.project_id.clone()),
            working_dir: template.and_then(|t| t.working_dir.clone()),
            auto_approve_level: Some(auto_approve_level),
            ..crate::storage::NewSession::new(id.clone())
        })
        .await
        .map_err(|e| CronError::Storage(format!("failed to create session for cron run: {e}")))?;
    let title = format!(
        "{} · {}",
        job_name,
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    if let Err(e) = session_store.update_title(&id, &title).await {
        // Roll back the just-created session instead of orphaning it.
        let _ = session_store.delete(&id).await;
        return Err(CronError::Storage(format!(
            "failed to title cron session: {e}"
        )));
    }
    Ok(id)
}

/// 校验 schedule 并计算距离现在最近的触发时间。创建与更新路径共用，
/// 保证"永不触发的 schedule"在两条路径上都被拒绝。
pub fn next_run_from_schedule(schedule: &str) -> Result<chrono::DateTime<chrono::Utc>, CronError> {
    CronSchedule::parse(schedule)?
        .next_after(chrono::Utc::now())
        .ok_or_else(|| CronError::InvalidSchedule("schedule has no upcoming fire time".into()))
}

/// 创建并持久化一个 cron job（ensure 语义）：name 是 job 的身份。
/// 同名 job 已存在时不新建、不改写，直接返回它（`created = false`），
/// 调用方始终拿到一个稳定 id——agent 跨 session 重复 create 同一个
/// 名字不会产生重复 job。要调整已有 job 请走 update。
///
/// 新 job 的路径：校验 schedule、计算 `next_run_at`、入库。
/// `SendMessage` 不显式绑定 session 时**不在此创建会话**——只捕获
/// per-run 模板（见 [`capture_session_template`]），每次触发才新建
/// 独立 session（见 [`spawn_run_session`]）。
///
/// `Kernel`（RPC 路径）与 cron tool 共用，保证行为一致。
pub async fn create_cron_job(
    store: &Arc<dyn CronStore>,
    follow: Option<&crate::storage::SessionInfo>,
    input: CreateCronJobInput,
    config_auto_approve: crate::permission::Level,
) -> Result<CreateCronJobOutcome, CronError> {
    // Ensure 语义短路：同名即命中，本次传入的参数不校验、不生效。
    if let Some(existing) = store.get_by_name(&input.name).await? {
        return Ok(CreateCronJobOutcome {
            job: existing,
            created: false,
        });
    }

    let next_run = next_run_from_schedule(&input.schedule)?;

    // SendMessage 不绑定固定 session → per-run：准备模板，触发时才建会话。
    // 调用方可自带模板（RPC/GUI）：working_dir/project 尊重其值，但权限
    // 等级一律按当前 config 重算（下限 caution）——不信任调用方给的等级。
    let action = match input.action {
        CronAction::SendMessage {
            session_id: None,
            content,
            session_template,
        } => {
            let mut tpl = session_template
                .unwrap_or_else(|| capture_session_template(follow, config_auto_approve));
            tpl.auto_approve_level = Some(
                config_auto_approve
                    .max(crate::permission::Level::Caution)
                    .as_str()
                    .to_string(),
            );
            CronAction::SendMessage {
                session_id: None,
                content,
                session_template: Some(tpl),
            }
        }
        other => other,
    };

    let now = chrono::Utc::now();
    let job = CronJob {
        id: CronJobId::new(),
        name: input.name,
        schedule: input.schedule,
        action,
        status: CronJobStatus::Active,
        created_at: now,
        updated_at: now,
        next_run_at: Some(next_run),
        last_run_at: None,
        run_count: 0,
        max_runs: input.max_runs.unwrap_or(UNLIMITED_MAX_RUNS),
        expires_at: input.expires_at.unwrap_or(NEVER_EXPIRES),
        last_error: None,
        // 空白串归一为"无闸门"（store 边界兜底，各入口不用各自防范）
        precheck: input.precheck.filter(|s| !s.trim().is_empty()),
    };

    if let Err(e) = store.create(&job).await {
        // 并发 create 撞名（唯一索引兜底）：返回竞态胜者，保持 ensure 语义。
        if matches!(e, CronError::DuplicateName(_)) {
            match store.get_by_name(&job.name).await {
                Ok(Some(existing)) => {
                    return Ok(CreateCronJobOutcome {
                        job: existing,
                        created: false,
                    });
                }
                Ok(None) => {}
                Err(fetch_err) => {
                    // 回退查询失败不能吞掉原始的撞名错误
                    tracing::warn!("cron name-conflict fallback fetch failed: {fetch_err}");
                }
            }
        }
        return Err(e);
    }

    Ok(CreateCronJobOutcome { job, created: true })
}

/// 成功执行的 shell 命令输出。
#[derive(Debug)]
pub struct ShellOutput {
    pub stdout: String,
    /// 命令以 `SHELL_COMPLETE_EXIT_CODE` 退出（任务请求自我完成）。
    pub self_complete: bool,
}

/// 构造与 cron worker 一致的加固 shell 命令（`run_shell_command` 与
/// `run_precheck` 共用）。
fn shell_command(
    command: &str,
    working_dir: Option<&str>,
    data_dir: &std::path::Path,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(working_dir.unwrap_or("."))
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("GIT_PAGER", "cat")
        .env("GIT_EDITOR", "true")
        .env("GIT_SEQUENCE_EDITOR", "true")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PAGER", "cat")
        .env("EDITOR", "true");
    crate::utils::env::inject_child_env(&mut cmd, Some(data_dir), None);
    cmd
}

/// 以与 cron worker 一致的加固环境执行 shell 命令。
/// 退出码 0 与 `SHELL_COMPLETE_EXIT_CODE` 都视为成功返回输出；
/// 其他非零退出返回 `CronError::ShellFailed`（含 stderr）。
///
/// 注入 yomi 标准环境变量（见 [`crate::utils::env::inject_child_env`]；
/// cron shell job 无会话，只有 `YOMI_DATA_DIR`）。
pub async fn run_shell_command(
    command: &str,
    working_dir: Option<&str>,
    data_dir: &std::path::Path,
) -> Result<ShellOutput, CronError> {
    let output = shell_command(command, working_dir, data_dir)
        .output()
        .await
        .map_err(CronError::Io)?;

    let stdout = || String::from_utf8_lossy(&output.stdout).to_string();
    match output.status.code() {
        Some(0) => Ok(ShellOutput {
            stdout: stdout(),
            self_complete: false,
        }),
        Some(SHELL_COMPLETE_EXIT_CODE) => Ok(ShellOutput {
            stdout: stdout(),
            self_complete: true,
        }),
        _ => Err(CronError::ShellFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        )),
    }
}
