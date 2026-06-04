use crate::app::Coordinator;
use crate::cron::scheduler::CronScheduler;
use crate::cron::store::CronStore;
use crate::cron::types::{CronAction, CronError, CronJob};
use crate::types::{ContentBlock, SessionId};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub struct CronWorker {
    coordinator: Arc<Coordinator>,
    task_rx: mpsc::Receiver<CronJob>,
    store: Arc<dyn CronStore>,
    scheduler: Option<Arc<CronScheduler>>,
}

impl CronWorker {
    pub fn new(
        coordinator: Arc<Coordinator>,
        task_rx: mpsc::Receiver<CronJob>,
        store: Arc<dyn CronStore>,
        scheduler: Option<Arc<CronScheduler>>,
    ) -> Self {
        Self {
            coordinator,
            task_rx,
            store,
            scheduler,
        }
    }

    pub async fn run(mut self) {
        while let Some(job) = self.task_rx.recv().await {
            let coordinator = Arc::clone(&self.coordinator);
            let store = Arc::clone(&self.store);
            let scheduler = self.scheduler.clone();

            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let result = Self::execute(&coordinator, &job).await;
                let elapsed = start.elapsed();

                let error = match &result {
                    Ok(()) => None,
                    Err(e) => {
                        tracing::error!("Cron job {} failed: {}", job.id.0, e);
                        Some(e.to_string())
                    }
                };

                // 记录执行结果（不计算 next_run，由 scheduler 维护）
                if let Err(e) = store.record_execution(&job.id, None, error).await {
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

        tracing::info!("Cron worker shutting down");
    }

    async fn execute(coordinator: &Coordinator, job: &CronJob) -> Result<(), CronError> {
        // 设置执行超时（默认 5 分钟）
        const TIMEOUT_SECS: u64 = 300;
        let timeout = Duration::from_secs(TIMEOUT_SECS);

        let result = tokio::time::timeout(timeout, async {
            match &job.action {
                CronAction::SendMessage {
                    session_id,
                    content,
                } => {
                    let sid = SessionId(session_id.clone());
                    // 如果 session 不在内存，尝试恢复
                    if coordinator.get_session(&sid).is_none() {
                        coordinator.restore_session(&sid).await?;
                    }
                    let text = Self::render_template(content);
                    let blocks = vec![ContentBlock::Text { text }];
                    coordinator.send_message(&sid, blocks).await?;
                }
                CronAction::Shell {
                    command,
                    working_dir,
                } => {
                    let output = tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(command)
                        .current_dir(working_dir.as_deref().unwrap_or("."))
                        .kill_on_drop(true)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .output()
                        .await?;
                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        return Err(CronError::ShellFailed(stderr.to_string()));
                    }
                }
                CronAction::Internal { .. } => {
                    return Err(CronError::UnsupportedAction("Internal".to_string()));
                }
            }
            Ok(())
        })
        .await;

        match result {
            Ok(r) => r,
            Err(_) => Err(CronError::Timeout(TIMEOUT_SECS)),
        }
    }

    /// 简单模板渲染
    fn render_template(template: &str) -> String {
        let now = Utc::now();
        template
            .replace("{{timestamp}}", &now.to_rfc3339())
            .replace("{{date}}", &now.format("%Y-%m-%d").to_string())
            .replace("{{time}}", &now.format("%H:%M:%S").to_string())
    }
}
