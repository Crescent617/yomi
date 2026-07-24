use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

pub use crate::types::CronJobId;

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
        /// 目标 session。创建 job 时可为空：kernel 会新建一个专用 session
        /// 并在持久化前回填，之后每次触发都发往同一个 session。
        session_id: Option<String>,
        /// 消息内容，支持模板变量：
        /// - {{timestamp}} — ISO8601 时间戳
        /// - {{date}} — YYYY-MM-DD
        /// - {{time}} — HH:MM:SS
        content: String,
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
    /// cron 表达式，如 "0 0 9 * * 1-5"（工作日 9:00，按本地时区解释）
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
    /// 最大执行次数（None = 无限）
    pub max_runs: Option<u32>,
    /// 过期时间（None = 永不过期）
    pub expires_at: Option<DateTime<Utc>>,
    /// 最近错误信息
    pub last_error: Option<String>,
}

/// 创建任务输入
#[derive(Debug, Clone, Deserialize)]
pub struct CreateCronJobInput {
    pub name: String,
    pub schedule: String,
    pub action: CronAction,
    pub max_runs: Option<u32>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// 更新任务输入（部分更新）
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateCronJobInput {
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub action: Option<CronAction>,
    pub status: Option<CronJobStatus>,
    pub max_runs: Option<u32>,
    pub expires_at: Option<DateTime<Utc>>,
    /// 显式清除 `max_runs`（设为 NULL，恢复无限次）
    #[serde(default)]
    pub clear_max_runs: bool,
    /// 显式清除 `expires_at`（设为 NULL，永不过期）
    #[serde(default)]
    pub clear_expires_at: bool,
    /// 用于 scheduler 内部更新 `next_run_at`
    #[serde(skip)]
    pub next_run_at: Option<DateTime<Utc>>,
}

/// Cron 表达式封装
pub struct CronSchedule {
    schedule: cron::Schedule,
    source: String,
}

impl CronSchedule {
    pub fn parse(expression: &str) -> Result<Self, CronError> {
        let trimmed = expression.trim();
        let parts: Vec<&str> = trimmed.split_whitespace().collect();

        // cron 0.15 requires 6 fields (seconds minutes hours days months weekdays).
        // Standard Unix cron uses 5 fields. Auto-prefix "0" for seconds when 5 fields are given.
        let expression = if parts.len() == 5 {
            format!("0 {trimmed}")
        } else {
            trimmed.to_string()
        };

        let schedule: cron::Schedule = expression
            .parse()
            .map_err(|e: cron::error::Error| CronError::InvalidSchedule(e.to_string()))?;
        Ok(Self {
            schedule,
            source: trimmed.to_string(),
        })
    }

    /// 计算下一次触发时间（从 from 之后开始）。
    ///
    /// cron 表达式按**本地时区**解释（如 `0 0 9 * * *` = 本地 9:00），
    /// 返回值统一转换为 UTC 便于存储与比较。不存在的本地时间（DST 春拨）
    /// 会被跳过。
    pub fn next_after(&self, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.schedule
            .after(&from.with_timezone(&Local))
            .map(|dt| dt.with_timezone(&Utc))
            // DST 秋拨时 cron 会把歧义 wall time 解析为 earlier occurrence，
            // 其绝对时间可能早于 from；过滤掉这些“过去”的结果。
            .find(|dt| *dt > from)
    }

    /// 计算 upcoming N 次触发时间（本地时区解释，返回 UTC）
    pub fn upcoming(&self, from: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
        self.schedule
            .after(&from.with_timezone(&Local))
            .map(|dt| dt.with_timezone(&Utc))
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
