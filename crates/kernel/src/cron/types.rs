use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

pub use crate::types::CronJobId;

/// `max_runs` sentinel meaning "no run limit".
pub const UNLIMITED_MAX_RUNS: u32 = 0;
/// `expires_at` sentinel meaning "never expires" (the zero timestamp).
pub const NEVER_EXPIRES: DateTime<Utc> = DateTime::UNIX_EPOCH;

/// 任务触发时要执行的动作
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "snake_case"
)]
pub enum CronAction {
    /// 向指定 Session 发送消息（触发 Agent 响应）
    SendMessage {
        /// 目标 session：有值时每次触发都发往同一个会话；为 None 时
        /// 每次触发都用 `session_template` 新建一个独立会话（运行后保留）。
        session_id: Option<String>,
        /// 消息内容，支持模板变量：
        /// - {{timestamp}} — ISO8601 时间戳
        /// - {{date}} — YYYY-MM-DD
        /// - {{time}} — HH:MM:SS
        content: String,
        /// `session_id` 为 None 时，每次触发新建 session 所用的模板——
        /// 在创建（或解绑）job 时捕获，见 [`crate::cron::capture_session_template`]。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_template: Option<CronSessionTemplate>,
    },
    /// 执行 Shell 命令
    Shell {
        command: String,
        working_dir: Option<String>,
    },
    /// 调用内部 API（预留扩展）
    Internal {
        endpoint: String,
        payload: serde_json::Value,
    },
}

/// per-run session 的创建模板：job 创建（或解绑）时从调用方 session 捕获，
/// 此后每次触发按它新建独立会话。字段为 None 表示无继承来源（如 RPC 路径），
/// 触发时按最小可用配置创建。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronSessionTemplate {
    /// 新建 session 的工作目录（跟随创建方 session）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// 新建 session 所属项目（跟随创建方 session）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::types::ProjectId>,
    /// 新建 session 的自动批准阈值（创建时按全局 config 快照，下限 caution）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_approve_level: Option<String>,
}

/// 定时任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronJobStatus {
    Active,
    Paused,
    Completed, // 达到 max_runs 或过期
    Failed,    // 连续失败超过阈值（预留）
}

impl std::str::FromStr for CronJobStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("Invalid cron job status: {s}")),
        }
    }
}

impl CronJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// 定时任务记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: CronJobId,
    pub name: String,
    /// cron 表达式，如 "0 0 9 * * 1-5"（工作日 9:00，按本地时区解释；
    /// 星期字段为 UNIX 约定：0 或 7=周日，1=周一 … 6=周六，也接受 mon/tue/... 缩写）
    pub schedule: String,
    pub action: CronAction,
    pub status: CronJobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 下次应该执行的时间（由 scheduler 维护）
    pub next_run_at: Option<DateTime<Utc>>,
    /// 最后一次执行时间
    pub last_run_at: Option<DateTime<Utc>>,
    /// 执行次数统计
    pub run_count: u32,
    /// 最大执行次数（[`UNLIMITED_MAX_RUNS`] = 无限）
    pub max_runs: u32,
    /// 过期时间（[`NEVER_EXPIRES`] = 永不过期）
    pub expires_at: DateTime<Utc>,
    /// 最近错误信息
    pub last_error: Option<String>,
}

impl CronJob {
    /// Whether the job has a run limit.
    pub fn has_max_runs(&self) -> bool {
        self.max_runs > UNLIMITED_MAX_RUNS
    }

    /// Whether the job can expire.
    pub fn has_expiry(&self) -> bool {
        self.expires_at > NEVER_EXPIRES
    }
}

/// 创建任务输入
#[derive(Debug, Clone, Deserialize)]
pub struct CreateCronJobInput {
    pub name: String,
    pub schedule: String,
    pub action: CronAction,
    /// `None` / `Some(0)` = 不限次数
    pub max_runs: Option<u32>,
    /// `None` = 永不过期
    pub expires_at: Option<DateTime<Utc>>,
}

/// `create_cron_job` 的结果。
///
/// `created = false` 表示撞名：返回的是**已存在**的 job，本次传入的
/// schedule/action 等参数均未生效——要调整已有 job 请走 update。
#[derive(Debug, Clone)]
pub struct CreateCronJobOutcome {
    pub job: CronJob,
    pub created: bool,
}

/// 更新任务输入（部分更新）
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateCronJobInput {
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub action: Option<CronAction>,
    pub status: Option<CronJobStatus>,
    /// `None` = 不变；`Some(0)` = 恢复不限次数
    pub max_runs: Option<u32>,
    /// `None` = 不变；`Some(NEVER_EXPIRES)` = 恢复永不过期
    pub expires_at: Option<DateTime<Utc>>,
    /// 用于 scheduler 内部更新 `next_run_at`
    #[serde(skip)]
    pub next_run_at: Option<DateTime<Utc>>,
}

/// Cron 表达式封装
pub struct CronSchedule {
    schedule: croner::Cron,
    source: String,
}

impl CronSchedule {
    pub fn parse(expression: &str) -> Result<Self, CronError> {
        let trimmed = expression.trim();

        // croner 默认解析器：秒/年字段可选（5/6 段通吃），星期字段为
        // POSIX/UNIX 约定（0 或 7=周日，1=周一 … 6=周六）。
        let schedule = trimmed
            .parse::<croner::Cron>()
            .map_err(|e: croner::errors::CronError| CronError::InvalidSchedule(e.to_string()))?;
        Ok(Self {
            schedule,
            source: trimmed.to_string(),
        })
    }

    /// 计算下一次触发时间（从 from 之后开始）。
    ///
    /// cron 表达式按**本地时区**解释（如 `0 0 9 * * *` = 本地 9:00），
    /// 返回值统一转换为 UTC 便于存储与比较。
    pub fn next_after(&self, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        // `dt > from` 过滤是承重的：DST 秋拨的重叠小时里，croner 会把
        // 歧义 wall time 解析为 earliest（其绝对时间可能早于 from），
        // 不滤掉 scheduler 会认为任务"已到期"而热重跑。
        self.schedule
            .iter_after(from.with_timezone(&Local))
            .map(|dt| dt.with_timezone(&Utc))
            .find(|dt| *dt > from)
    }

    /// 计算 upcoming N 次触发时间（本地时区解释，返回 UTC）
    pub fn upcoming(&self, from: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
        self.schedule
            .iter_after(from.with_timezone(&Local))
            .map(|dt| dt.with_timezone(&Utc))
            // 同 next_after：过滤 DST 秋拨歧义产生的"过去"结果
            .filter(|dt| *dt > from)
            .take(n)
            .collect()
    }

    pub fn source(&self) -> &str {
        &self.source
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("Invalid cron schedule: {0}")]
    InvalidSchedule(String),

    #[error("Job not found: {0}")]
    JobNotFound(String),

    #[error("Cron job name already exists: {0}")]
    DuplicateName(String),

    #[error("Shell command failed: {0}")]
    ShellFailed(String),

    #[error("Session error: {0}")]
    Session(#[from] crate::types::KernelError),

    #[error("Unsupported action: {0}")]
    UnsupportedAction(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Cron job execution timed out after {0} seconds")]
    Timeout(u64),
}

impl From<sqlx::Error> for CronError {
    fn from(e: sqlx::Error) -> Self {
        CronError::Storage(e.to_string())
    }
}

impl From<CronError> for crate::types::KernelError {
    fn from(e: CronError) -> Self {
        crate::types::KernelError::storage(e.to_string())
    }
}

impl From<std::str::Utf8Error> for CronError {
    fn from(e: std::str::Utf8Error) -> Self {
        CronError::ShellFailed(e.to_string())
    }
}

/// 渲染 cron 模板中的变量占位符（按本地时区取值）：
/// - `{{timestamp}}` → ISO8601 时间戳（含本地时区偏移）
/// - `{{date}}` → YYYY-MM-DD
/// - `{{time}}` → HH:MM:SS
pub fn render_template(template: &str) -> String {
    let now = chrono::Local::now();
    template
        .replace("{{timestamp}}", &now.to_rfc3339())
        .replace("{{date}}", &now.format("%Y-%m-%d").to_string())
        .replace("{{time}}", &now.format("%H:%M:%S").to_string())
}
