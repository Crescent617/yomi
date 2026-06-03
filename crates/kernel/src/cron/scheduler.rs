use crate::cron::store::CronStore;
use crate::cron::types::{CronError, CronJob, CronJobId, CronJobStatus, CronSchedule};
use chrono::{DateTime, Utc};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

/// 调度队列中的条目（小顶堆）
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledEntry {
    next_run: DateTime<Utc>,
    job_id: CronJobId,
}

impl Ord for ScheduledEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.next_run.cmp(&other.next_run)
    }
}

impl PartialOrd for ScheduledEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 轻量级 cron 调度引擎
///
/// 维护一个按 next_run_at 排序的小顶堆，精确 sleep 到触发点。
/// 新任务加入/更新/删除时通过 Notify 打断 sleep 重新加载。
pub struct CronScheduler {
    store: Arc<dyn CronStore>,
    task_tx: mpsc::Sender<CronJob>,
    /// 按 next_run_at 排序的小顶堆
    queue: Arc<RwLock<BinaryHeap<ScheduledEntry>>>,
    /// 任务 ID -> 完整 CronJob 的缓存
    jobs: Arc<RwLock<HashMap<CronJobId, CronJob>>>,
    /// 正在执行中的任务 ID，防止重复调度
    running: Arc<RwLock<HashSet<CronJobId>>>,
    /// 有新任务加入时通知调度循环重新计算
    /// 使用 Notify::notify_waiters 确保通知不丢失
    notify: Arc<tokio::sync::Notify>,
    pub(crate) shutdown: CancellationToken,
}

impl CronScheduler {
    pub fn new(
        store: Arc<dyn CronStore>,
        task_tx: mpsc::Sender<CronJob>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            store,
            task_tx,
            queue: Arc::new(RwLock::new(BinaryHeap::new())),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(HashSet::new())),
            notify: Arc::new(tokio::sync::Notify::new()),
            shutdown,
        }
    }

    /// 启动调度主循环
    pub async fn run(self: Arc<Self>) {
        if let Err(e) = self.load_jobs().await {
            tracing::error!("Failed to load cron jobs: {}", e);
        }

        loop {
            let (sleep_duration, has_jobs) = {
                let queue = self.queue.read().await;
                if let Some(entry) = queue.peek() {
                    let now = Utc::now();
                    if entry.next_run <= now {
                        // 有任务已经到期，立即处理
                        (std::time::Duration::from_secs(0), true)
                    } else {
                        let dur = (entry.next_run - now)
                            .to_std()
                            .unwrap_or(std::time::Duration::from_secs(1));
                        (dur, true)
                    }
                } else {
                    // 没有任务，sleep 60 秒后重新检查
                    (std::time::Duration::from_secs(60), false)
                }
            };

            tokio::select! {
                biased;
                () = self.shutdown.cancelled() => {
                    tracing::info!("Cron scheduler shutting down");
                    break;
                }
                () = tokio::time::sleep(sleep_duration), if has_jobs => {
                    if let Err(e) = self.fire_due_jobs().await {
                        tracing::error!("Failed to fire due jobs: {}", e);
                    }
                }
                () = self.notify.notified() => {
                    tracing::debug!("Cron scheduler notified to reload jobs");
                    if let Err(e) = self.load_jobs().await {
                        tracing::error!("Failed to reload cron jobs: {}", e);
                    }
                }
            }
        }
    }

    /// 外部调用：任务变更后通知调度器重新加载
    pub fn reload(&self) {
        self.notify.notify_waiters();
    }

    /// 任务执行完成后，重新将任务加入调度队列
    pub async fn job_finished(&self, job_id: &CronJobId) {
        let mut running = self.running.write().await;
        running.remove(job_id);
        drop(running);

        // 重新加载该任务的调度时间
        if let Ok(Some(job)) = self.store.get(job_id).await {
            if matches!(job.status, CronJobStatus::Active) {
                if let Ok(schedule) = CronSchedule::parse(&job.schedule) {
                    let next = schedule.next_after(Utc::now());
                    if let Some(next) = next {
                        let mut queue = self.queue.write().await;
                        let mut jobs = self.jobs.write().await;

                        queue.push(ScheduledEntry {
                            next_run: next,
                            job_id: job_id.clone(),
                        });
                        if let Some(j) = jobs.get_mut(job_id) {
                            j.next_run_at = Some(next);
                        }
                    }
                }
            }
        }
    }

    /// 从数据库加载所有 active 任务，计算 next_run_at
    async fn load_jobs(&self) -> Result<(), CronError> {
        let active_jobs = self.store.list_active().await?;
        let now = Utc::now();

        let mut queue = self.queue.write().await;
        let mut jobs = self.jobs.write().await;

        queue.clear();
        jobs.clear();

        for mut job in active_jobs {
            // 如果 next_run_at 为空或已过期，重新计算
            let next_run = match job.next_run_at {
                Some(t) if t > now => Some(t),
                _ => {
                    // 重新计算 next_run
                    match CronSchedule::parse(&job.schedule) {
                        Ok(schedule) => {
                            let next = schedule.next_after(now);
                            // 更新数据库中的 next_run_at
                            if let Err(e) = self
                                .store
                                .update(
                                    &job.id,
                                    &crate::cron::types::UpdateCronJobInput {
                                        next_run_at: next,
                                        ..Default::default()
                                    },
                                )
                                .await
                            {
                                tracing::warn!(
                                    "Failed to update next_run_at for job {}: {}",
                                    job.id.0,
                                    e
                                );
                            }
                            next
                        }
                        Err(e) => {
                            tracing::error!("Invalid schedule for job {}: {}", job.id.0, e);
                            None
                        }
                    }
                }
            };

            if let Some(next) = next_run {
                job.next_run_at = Some(next);
                queue.push(ScheduledEntry {
                    next_run: next,
                    job_id: job.id.clone(),
                });
                jobs.insert(job.id.clone(), job);
            }
        }

        tracing::info!("Loaded {} active cron jobs", jobs.len());
        Ok(())
    }

    /// 触发所有到期的任务
    async fn fire_due_jobs(&self) -> Result<(), CronError> {
        let now = Utc::now();
        let mut due_jobs = Vec::new();

        // 收集所有到期且未在执行中的任务
        {
            let queue = self.queue.read().await;
            let jobs = self.jobs.read().await;
            let running = self.running.read().await;

            // 由于 BinaryHeap 只能 peek 堆顶，我们遍历所有条目
            // 对于少量任务（<1000）这是可接受的
            for entry in queue.iter() {
                if entry.next_run > now {
                    break;
                }
                if running.contains(&entry.job_id) {
                    continue;
                }
                if let Some(job) = jobs.get(&entry.job_id) {
                    due_jobs.push(job.clone());
                }
            }
        }

        for job in due_jobs {
            // 检查是否过期或达到最大执行次数
            if let Some(expires_at) = job.expires_at {
                if now >= expires_at {
                    tracing::info!("Cron job {} expired", job.id.0);
                    self.store
                        .update(
                            &job.id,
                            &crate::cron::types::UpdateCronJobInput {
                                status: Some(CronJobStatus::Completed),
                                ..Default::default()
                            },
                        )
                        .await
                        .ok();
                    self.remove_from_queue(&job.id).await;
                    continue;
                }
            }

            if let Some(max_runs) = job.max_runs {
                if job.run_count >= max_runs {
                    tracing::info!("Cron job {} reached max runs", job.id.0);
                    self.store
                        .update(
                            &job.id,
                            &crate::cron::types::UpdateCronJobInput {
                                status: Some(CronJobStatus::Completed),
                                ..Default::default()
                            },
                        )
                        .await
                        .ok();
                    self.remove_from_queue(&job.id).await;
                    continue;
                }
            }

            // 标记为执行中，防止重复调度
            {
                let mut running = self.running.write().await;
                running.insert(job.id.clone());
            }

            // 从队列中移除（避免重复触发）
            self.remove_from_queue(&job.id).await;

            // 发送任务到 worker
            if let Err(e) = self.task_tx.send(job.clone()).await {
                tracing::error!("Failed to send cron job to worker: {}", e);
                // 发送失败，从 running 中移除
                let mut running = self.running.write().await;
                running.remove(&job.id);
            }
        }

        Ok(())
    }

    /// 从队列中移除指定任务
    ///
    /// BinaryHeap 不支持直接移除，需要重建。
    /// 对于少量任务（<1000）这是 O(N) 但常数很小。
    /// 如果任务数增长到数千，应考虑使用 BTreeMap + HashSet 的混合结构。
    async fn remove_from_queue(&self, job_id: &CronJobId) {
        let mut queue = self.queue.write().await;
        let mut entries: Vec<_> = std::mem::take(&mut *queue).into_vec();
        entries.retain(|e| e.job_id != *job_id);
        *queue = entries.into();
    }
}
