pub mod scheduler;
pub mod store;
#[cfg(test)]
mod tests;
pub mod types;
pub mod worker;

pub use scheduler::CronScheduler;
pub use store::{CronStore, SqliteCronStore};
pub use types::{
    CreateCronJobInput, CronAction, CronError, CronJob, CronJobId, CronJobStatus, CronSchedule,
    UpdateCronJobInput, NEVER_EXPIRES, UNLIMITED_MAX_RUNS,
};
pub use worker::CronWorker;

use async_trait::async_trait;
use std::sync::Arc;

/// 执行 cron action 的接口。`CronWorker` 只依赖此 trait，不依赖 `Kernel`。
///
/// 这确保 cron 子系统与上层协调器的解耦：
/// - `Kernel` 负责 session 管理、消息发送、Shell 执行等
/// - `CronWorker` 只负责调度、超时、结果记录
#[async_trait]
pub trait CronExecutor: Send + Sync {
    async fn execute_cron_action(
        &self,
        action: &CronAction,
    ) -> Result<CronActionOutcome, CronError>;
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
}

/// 确保 `SendMessage` action 绑定了具体 session。
///
/// `session_id` 为空时新建一个专用 session（标题取 job 名，权限等级为
/// 默认 safe），并把新 id 回填进 action；之后每次触发都发往同一个 session。
///
/// `follow` 为调用方所在 session 的元信息（tool 场景）：新 session 会继承其
/// `working_dir` 与 `project_id`；`model_key` 不继承，保持默认模型。
/// 其他 action 原样返回。
pub async fn ensure_action_session(
    action: CronAction,
    job_name: &str,
    session_store: &Arc<dyn crate::storage::SessionStore>,
    follow: Option<&crate::storage::SessionInfo>,
) -> Result<CronAction, CronError> {
    let CronAction::SendMessage {
        session_id: None,
        content,
    } = action
    else {
        return Ok(action);
    };

    let id = crate::types::SessionId::new();
    session_store
        .create(
            &id,
            follow.and_then(|i| i.project_id.as_ref()),
            follow.and_then(|i| i.working_dir.as_deref()),
            Some(crate::permission::Level::default().as_str()),
            None,
            None,
        )
        .await
        .map_err(|e| CronError::Storage(format!("failed to create session for cron job: {e}")))?;
    if let Err(e) = session_store.update_title(&id, job_name).await {
        // Roll back the just-created session instead of orphaning it.
        let _ = session_store.delete(&id).await;
        return Err(CronError::Storage(format!(
            "failed to title cron session: {e}"
        )));
    }

    Ok(CronAction::SendMessage {
        session_id: Some(id.0.to_string()),
        content,
    })
}

/// 校验 schedule 并计算距离现在最近的触发时间。创建与更新路径共用，
/// 保证"永不触发的 schedule"在两条路径上都被拒绝。
pub fn next_run_from_schedule(schedule: &str) -> Result<chrono::DateTime<chrono::Utc>, CronError> {
    CronSchedule::parse(schedule)?
        .next_after(chrono::Utc::now())
        .ok_or_else(|| CronError::InvalidSchedule("schedule has no upcoming fire time".into()))
}

/// 提取 `SendMessage` action 绑定的 session（如有）。
pub fn action_session_id(action: &CronAction) -> Option<crate::types::SessionId> {
    match action {
        CronAction::SendMessage {
            session_id: Some(sid),
            ..
        } => Some(crate::types::SessionId::from(sid.clone())),
        _ => None,
    }
}

/// 回滚为 cron job 绑定后却未能成功入库的专用 session（尽力而为）。
/// 只应传入**新绑定**的 session——用户显式提供的 session 绝不可删。
pub async fn rollback_bound_session(
    session_store: &Arc<dyn crate::storage::SessionStore>,
    session: Option<crate::types::SessionId>,
) {
    if let Some(sid) = session {
        let _ = session_store.delete(&sid).await;
    }
}

/// 创建并持久化一个 cron job：校验 schedule、按需绑定专用 session、
/// 计算 `next_run_at`、入库。若入库失败，回滚刚绑定的 session。
///
/// `Kernel`（RPC 路径）与 cron tool 共用，保证行为一致。
pub async fn create_cron_job(
    store: &Arc<dyn CronStore>,
    session_store: Option<&Arc<dyn crate::storage::SessionStore>>,
    follow: Option<&crate::storage::SessionInfo>,
    input: CreateCronJobInput,
) -> Result<CronJob, CronError> {
    let next_run = next_run_from_schedule(&input.schedule)?;

    // `SendMessage` without a session gets a dedicated new session bound now,
    // so every fire lands in the same conversation.
    let needs_new_session = matches!(
        input.action,
        CronAction::SendMessage {
            session_id: None,
            ..
        }
    );
    let action = if needs_new_session {
        let session_store = session_store.ok_or_else(|| {
            CronError::Storage("session store not available; pass session_id explicitly".into())
        })?;
        ensure_action_session(input.action, &input.name, session_store, follow).await?
    } else {
        input.action
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
    };

    if let Err(e) = store.create(&job).await {
        // Best-effort rollback of the dedicated session bound above (never
        // a user-supplied session — needs_new_session gates it).
        if needs_new_session {
            if let Some(session_store) = session_store {
                rollback_bound_session(session_store, action_session_id(&job.action)).await;
            }
        }
        return Err(e);
    }

    Ok(job)
}

/// 成功执行的 shell 命令输出。
#[derive(Debug)]
pub struct ShellOutput {
    pub stdout: String,
    /// 命令以 `SHELL_COMPLETE_EXIT_CODE` 退出（任务请求自我完成）。
    pub self_complete: bool,
}

/// 以与 cron worker 一致的加固环境执行 shell 命令。
/// 退出码 0 与 `SHELL_COMPLETE_EXIT_CODE` 都视为成功返回输出；
/// 其他非零退出返回 `CronError::ShellFailed`（含 stderr）。
pub async fn run_shell_command(
    command: &str,
    working_dir: Option<&str>,
) -> Result<ShellOutput, CronError> {
    let output = tokio::process::Command::new("sh")
        .arg("-c")
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
        .env("EDITOR", "true")
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
