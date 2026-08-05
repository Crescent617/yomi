use crate::cron::scheduler::CronScheduler;
use crate::cron::store::CronStore;
use crate::cron::types::{CronError, CronJob};
use crate::cron::{CronActionOutcome, CronExecutor};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 单次执行超时（worker 与 trigger 共用）
pub(crate) const EXECUTION_TIMEOUT_SECS: u64 = 300;

pub struct CronWorker {
    executor: Arc<dyn CronExecutor>,
    task_rx: mpsc::Receiver<CronJob>,
    store: Arc<dyn CronStore>,
    scheduler: Option<Arc<CronScheduler>>,
}

impl CronWorker {
    pub fn new(
        executor: Arc<dyn CronExecutor>,
        task_rx: mpsc::Receiver<CronJob>,
        store: Arc<dyn CronStore>,
        scheduler: Option<Arc<CronScheduler>>,
    ) -> Self {
        Self {
            executor,
            task_rx,
            store,
            scheduler,
        }
    }

    pub async fn run(mut self, token: CancellationToken) {
        loop {
            tokio::select! {
                biased;
                () = token.cancelled() => {
                    tracing::info!("Cron worker shutting down");
                    break;
                }
                Some(job) = self.task_rx.recv() => {
                    let executor = Arc::clone(&self.executor);
                    let store = Arc::clone(&self.store);
                    let scheduler = self.scheduler.clone();

                    tokio::spawn(async move {
                        let start = std::time::Instant::now();
                        let result = Self::execute(&*executor, &job).await;
                        let elapsed = start.elapsed();
                        Self::finalize(store, scheduler, &job, result).await;
                        tracing::info!("Cron job {} executed in {:?}", job.id.0, elapsed);
                    });
                }
                else => {
                    tracing::info!("Cron worker: task channel closed");
                    break;
                }
            }
        }

        tracing::info!("Cron worker shut down");
    }

    async fn execute(
        executor: &dyn CronExecutor,
        job: &CronJob,
    ) -> Result<CronActionOutcome, CronError> {
        let timeout = Duration::from_secs(EXECUTION_TIMEOUT_SECS);

        let result = tokio::time::timeout(timeout, executor.execute_cron_action(&job.action)).await;

        match result {
            Ok(r) => r,
            Err(_) => Err(CronError::Timeout(EXECUTION_TIMEOUT_SECS)),
        }
    }

    /// 记录执行结果并把任务交还调度器。`SelfComplete` 直接完成任务
    /// （不再入队），其余结果走 `job_finished` 按既有规则重新调度。
    async fn finalize(
        store: Arc<dyn CronStore>,
        scheduler: Option<Arc<CronScheduler>>,
        job: &CronJob,
        result: Result<CronActionOutcome, CronError>,
    ) {
        let (error, self_complete) = match &result {
            Ok(outcome) => (None, matches!(outcome, CronActionOutcome::SelfComplete)),
            Err(e) => {
                tracing::error!("Cron job {} failed: {}", job.id.0, e);
                (Some(e.to_string()), false)
            }
        };

        // 记录执行结果（只更新 run_count / last_run_at / last_error，
        // next_run_at 由 scheduler 统一管理）
        if let Err(e) = store.record_execution(&job.id, error).await {
            tracing::error!("Failed to record cron execution: {}", e);
        }

        if let Some(ref sched) = scheduler {
            if self_complete {
                sched.complete_job(&job.id).await;
            } else {
                sched.job_finished(&job.id).await;
            }
        }
    }
}

#[cfg(test)]
#[path = "worker_test.rs"]
mod tests;
