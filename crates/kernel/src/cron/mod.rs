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
    UpdateCronJobInput,
};
pub use worker::CronWorker;

use async_trait::async_trait;

/// 执行 cron action 的接口。`CronWorker` 只依赖此 trait，不依赖 `Coordinator`。
///
/// 这确保 cron 子系统与上层协调器的解耦：
/// - `Coordinator` 负责 session 管理、消息发送、Shell 执行等
/// - `CronWorker` 只负责调度、超时、结果记录
#[async_trait]
pub trait CronExecutor: Send + Sync {
    async fn execute_cron_action(&self, action: &CronAction) -> Result<(), CronError>;
}
