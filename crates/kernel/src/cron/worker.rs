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
    /// precheck 闸门命令的执行环境（`YOMI_DATA_DIR` 注入）。
    data_dir: std::path::PathBuf,
}

impl CronWorker {
    pub fn new(
        executor: Arc<dyn CronExecutor>,
        task_rx: mpsc::Receiver<CronJob>,
        store: Arc<dyn CronStore>,
        scheduler: Option<Arc<CronScheduler>>,
        data_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            executor,
            task_rx,
            store,
            scheduler,
            data_dir,
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
                    let data_dir = self.data_dir.clone();

                    tokio::spawn(async move {
                        let start = std::time::Instant::now();
                        let result = Self::execute_gated(&*executor, &job, &data_dir).await;
                        let elapsed = start.elapsed();
                        let skipped =
                            matches!(result, Ok(CronActionOutcome::Skipped));
                        Self::finalize(store, scheduler, &job, result).await;
                        if skipped {
                            tracing::info!("Cron job {} gate-checked in {:?}", job.id.0, elapsed);
                        } else {
                            tracing::info!("Cron job {} executed in {:?}", job.id.0, elapsed);
                        }
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

    /// precheck 闸门 → 放行才执行 action。闸门关闭返回
    /// [`CronActionOutcome::Skipped`]（本次不算一次执行）；放行时把
    /// 传感器 stdout 追加进 `send_message` 的消息体（agent 不必重跑检查）。
    async fn execute_gated(
        executor: &dyn CronExecutor,
        job: &CronJob,
        data_dir: &std::path::Path,
    ) -> Result<CronActionOutcome, CronError> {
        let Some(precheck) = &job.precheck else {
            return Self::execute(executor, job).await;
        };
        match crate::cron::run_precheck(precheck, job.precheck_working_dir(), data_dir).await {
            crate::cron::PrecheckOutcome::Skip => {
                tracing::info!("Cron job {} skipped by precheck gate", job.id.0);
                Ok(CronActionOutcome::Skipped)
            }
            crate::cron::PrecheckOutcome::Fire(stdout) => {
                let mut job = job.clone();
                if !stdout.is_empty() {
                    if let crate::cron::CronAction::SendMessage { content, .. } = &mut job.action {
                        *content = crate::cron::append_sensor_output(content, &stdout);
                    }
                }
                Self::execute(executor, &job).await
            }
        }
    }

    async fn execute(
        executor: &dyn CronExecutor,
        job: &CronJob,
    ) -> Result<CronActionOutcome, CronError> {
        let timeout = Duration::from_secs(EXECUTION_TIMEOUT_SECS);

        let result = tokio::time::timeout(timeout, executor.execute_cron_action(job)).await;

        match result {
            Ok(r) => r,
            Err(_) => Err(CronError::Timeout(EXECUTION_TIMEOUT_SECS)),
        }
    }

    /// 记录执行结果并把任务交还调度器。`SelfComplete` 直接完成任务
    /// （不再入队），`Skipped`（precheck 闸门关闭）不算一次执行——
    /// 不写执行记录，其余结果走 `job_finished` 按既有规则重新调度。
    async fn finalize(
        store: Arc<dyn CronStore>,
        scheduler: Option<Arc<CronScheduler>>,
        job: &CronJob,
        result: Result<CronActionOutcome, CronError>,
    ) {
        let (error, self_complete, skipped) = match &result {
            Ok(outcome) => (
                None,
                matches!(outcome, CronActionOutcome::SelfComplete),
                matches!(outcome, CronActionOutcome::Skipped),
            ),
            Err(e) => {
                tracing::error!("Cron job {} failed: {}", job.id.0, e);
                (Some(e.to_string()), false, false)
            }
        };

        // 记录执行结果（只更新 run_count / last_run_at / last_error，
        // next_run_at 由 scheduler 统一管理）。闸门跳过的触发不是一次
        // 执行：run_count 不增（max_runs 只计真正放行的运行）、
        // last_run_at/last_error 不动。
        if !skipped {
            if let Err(e) = store.record_execution(&job.id, error).await {
                tracing::error!("Failed to record cron execution: {}", e);
            }
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
