use crate::cron::scheduler::CronScheduler;
use crate::cron::store::CronStore;
use crate::cron::types::{CronError, CronJob};
use crate::cron::CronExecutor;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct CronWorker {
    executor: Arc<dyn CronExecutor>,
    task_rx: mpsc::Receiver<CronJob>,
    store: Arc<dyn CronStore>,
    scheduler: Option<Arc<CronScheduler>>,
    shutdown: CancellationToken,
}

impl CronWorker {
    pub fn new(
        executor: Arc<dyn CronExecutor>,
        task_rx: mpsc::Receiver<CronJob>,
        store: Arc<dyn CronStore>,
        scheduler: Option<Arc<CronScheduler>>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            executor,
            task_rx,
            store,
            scheduler,
            shutdown,
        }
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                () = self.shutdown.cancelled() => {
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

                        let error = match &result {
                            Ok(()) => None,
                            Err(e) => {
                                tracing::error!("Cron job {} failed: {}", job.id.0, e);
                                Some(e.to_string())
                            }
                        };

                        // 记录执行结果（只更新 run_count / last_run_at / last_error，
                        // next_run_at 由 scheduler 的 job_finished 统一管理）
                        if let Err(e) = store.record_execution(&job.id, error).await {
                            tracing::error!("Failed to record cron execution: {}", e);
                        }

                        // 通知 scheduler 任务已完成，可以重新调度
                        if let Some(ref sched) = scheduler {
                            sched.job_finished(&job.id).await;
                        }

                        tracing::info!(
                            "Cron job {} executed in {:?}: {:?}",
                            job.id.0,
                            elapsed,
                            result
                        );
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

    async fn execute(executor: &dyn CronExecutor, job: &CronJob) -> Result<(), CronError> {
        const TIMEOUT_SECS: u64 = 300;
        let timeout = Duration::from_secs(TIMEOUT_SECS);

        let result = tokio::time::timeout(timeout, executor.execute_cron_action(&job.action)).await;

        match result {
            Ok(r) => r,
            Err(_) => Err(CronError::Timeout(TIMEOUT_SECS)),
        }
    }
}
