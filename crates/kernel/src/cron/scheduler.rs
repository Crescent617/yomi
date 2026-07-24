use crate::cron::store::CronStore;
use crate::cron::types::{CronError, CronJob, CronJobId, CronJobStatus, CronSchedule};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

/// 轻量级 cron 调度引擎
///
/// 维护一个按 `next_run_at` 排序的调度队列（`BTreeMap`），精确 sleep 到触发点。
/// 新任务加入/更新/删除时通过 watch channel 打断 sleep 重新加载。
pub struct CronScheduler {
    store: Arc<dyn CronStore>,
    task_tx: mpsc::Sender<CronJob>,
    /// 按 `next_run_at` 排序的调度队列: `next_run` -> [`job_ids`]
    queue: Arc<RwLock<BTreeMap<DateTime<Utc>, Vec<CronJobId>>>>,
    /// 任务 ID -> 完整 `CronJob` 的缓存
    jobs: Arc<RwLock<HashMap<CronJobId, CronJob>>>,
    /// 正在执行中的任务 ID，防止重复调度
    running: Arc<RwLock<HashSet<CronJobId>>>,
    /// 有新任务变更时通知调度循环重新加载（watch channel 保证不丢信号）
    reload_tx: tokio::sync::watch::Sender<u64>,
}

impl CronScheduler {
    pub fn new(store: Arc<dyn CronStore>, task_tx: mpsc::Sender<CronJob>) -> Self {
        let (reload_tx, _reload_rx) = tokio::sync::watch::channel(0u64);
        Self {
            store,
            task_tx,
            queue: Arc::new(RwLock::new(BTreeMap::new())),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(HashSet::new())),
            reload_tx,
        }
    }

    /// 启动调度主循环
    pub async fn run(self: Arc<Self>, token: CancellationToken) {
        // 先订阅再加载，避免加载期间的 reload 信号丢失
        let mut reload_rx = self.reload_tx.subscribe();

        if let Err(e) = self.load_jobs().await {
            tracing::error!("Failed to load cron jobs: {e}");
        }

        loop {
            let (sleep_duration, has_jobs) = {
                let queue = self.queue.read().await;
                if let Some((next_run, _)) = queue.first_key_value() {
                    let now = Utc::now();
                    if *next_run <= now {
                        // 有任务已经到期，立即处理
                        (std::time::Duration::from_secs(0), true)
                    } else {
                        let dur = (*next_run - now)
                            .to_std()
                            .unwrap_or(std::time::Duration::from_secs(1));
                        (dur, true)
                    }
                } else {
                    // 没有任务，sleep 60 秒后重新检查
                    (std::time::Duration::from_mins(1), false)
                }
            };

            tokio::select! {
                biased;
                () = token.cancelled() => {
                    tracing::info!("Cron scheduler shutting down");
                    break;
                }
                () = tokio::time::sleep(sleep_duration), if has_jobs => {
                    if let Err(e) = self.fire_due_jobs().await {
                        tracing::error!("Failed to fire due jobs: {e}");
                    }
                }
                _ = reload_rx.changed() => {
                    let _ = reload_rx.borrow_and_update();
                    tracing::debug!("Cron scheduler reloading jobs");
                    if let Err(e) = self.load_jobs().await {
                        tracing::error!("Failed to reload cron jobs: {e}");
                    }
                }
            }
        }
    }

    /// 外部调用：任务变更后通知调度器重新加载
    pub fn reload(&self) {
        self.reload_tx.send_modify(|v| *v += 1);
    }

    /// 任务执行完成后，重新将任务加入调度队列。
    /// 从数据库读取最新状态，检查 `run_count` / `expires_at` / `status`，决定是否继续调度。
    pub async fn job_finished(&self, job_id: &CronJobId) {
        let mut running = self.running.write().await;
        running.remove(job_id);
        drop(running);

        // 移除旧的 queue entry，避免重复
        self.remove_job(job_id).await;

        let now = Utc::now();
        let job = match self.store.get(job_id).await {
            Ok(Some(j)) => j,
            Ok(None) => {
                tracing::warn!("Cron job {} not found in store after execution", job_id.0);
                return;
            }
            Err(e) => {
                tracing::error!("Failed to reload cron job {}: {}", job_id.0, e);
                return;
            }
        };

        // 非 active 状态直接丢弃，不重新入队
        if !matches!(job.status, CronJobStatus::Active) {
            return;
        }

        // 过期
        if let Some(expires_at) = job.expires_at {
            if now >= expires_at {
                self.complete_job(job_id).await;
                return;
            }
        }

        // 已达最大执行次数
        if let Some(max_runs) = job.max_runs {
            if job.run_count >= max_runs {
                self.complete_job(job_id).await;
                return;
            }
        }

        // 重新计算 next_run 并更新数据库 + 缓存
        if let Ok(schedule) = CronSchedule::parse(&job.schedule) {
            let next = schedule.next_after(now);
            if let Some(next) = next {
                if let Err(e) = self
                    .store
                    .update(
                        job_id,
                        &crate::cron::types::UpdateCronJobInput {
                            next_run_at: Some(next),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    tracing::warn!("Failed to update next_run_at for job {}: {}", job_id.0, e);
                }

                let mut queue = self.queue.write().await;
                let mut jobs = self.jobs.write().await;
                queue.entry(next).or_default().push(job_id.clone());
                if let Some(j) = jobs.get_mut(job_id) {
                    j.next_run_at = Some(next);
                } else {
                    // load_jobs 可能已清空缓存，直接插入完整 job 保持 queue/jobs 一致
                    let mut job = job;
                    job.next_run_at = Some(next);
                    jobs.insert(job_id.clone(), job);
                }
            }
        }
    }

    /// 从数据库加载所有 active 任务，计算 `next_run_at`
    async fn load_jobs(&self) -> Result<(), CronError> {
        let active_jobs = self.store.list_active().await?;
        let now = Utc::now();

        let mut queue = self.queue.write().await;
        let mut jobs = self.jobs.write().await;

        queue.clear();
        jobs.clear();

        for mut job in active_jobs {
            let next_run = Self::compute_next_run(&job, now, &self.store).await;
            if let Some(next) = next_run {
                job.next_run_at = Some(next);
                queue.entry(next).or_default().push(job.id.clone());
                jobs.insert(job.id.clone(), job);
            }
        }

        tracing::info!("Loaded {} active cron jobs", jobs.len());
        Ok(())
    }

    /// 辅助函数：计算 job 的 `next_run`，如果缺失/过期则重新计算并回写数据库。
    async fn compute_next_run(
        job: &CronJob,
        now: DateTime<Utc>,
        store: &Arc<dyn CronStore>,
    ) -> Option<DateTime<Utc>> {
        match job.next_run_at {
            Some(t) if t > now => Some(t),
            _ => match CronSchedule::parse(&job.schedule) {
                Ok(schedule) => {
                    let next = schedule.next_after(now);
                    if let Err(e) = store
                        .update(
                            &job.id,
                            &crate::cron::types::UpdateCronJobInput {
                                next_run_at: next,
                                ..Default::default()
                            },
                        )
                        .await
                    {
                        tracing::warn!("Failed to update next_run_at for job {}: {}", job.id.0, e);
                    }
                    next
                }
                Err(e) => {
                    tracing::error!("Invalid schedule for job {}: {}", job.id.0, e);
                    None
                }
            },
        }
    }

    /// 触发所有到期的任务
    async fn fire_due_jobs(&self) -> Result<(), CronError> {
        let now = Utc::now();
        let mut due_jobs = Vec::new();
        let mut stale = Vec::new();

        // 收集所有到期且未在执行中的任务
        {
            let queue = self.queue.read().await;
            let jobs = self.jobs.read().await;
            let running = self.running.read().await;

            for (next_run, job_ids) in queue.iter() {
                if *next_run > now {
                    break;
                }
                for job_id in job_ids {
                    if running.contains(job_id) {
                        // 执行中的 job 被 reload 重新入队后到期：entry 已过期，
                        // 丢弃它（job_finished 会重新入队），避免调度循环空转
                        stale.push(job_id.clone());
                        continue;
                    }
                    if let Some(job) = jobs.get(job_id) {
                        due_jobs.push(job.clone());
                    }
                }
            }
        }

        for job_id in stale {
            self.remove_job(&job_id).await;
        }

        for job in due_jobs {
            // 检查是否过期或达到最大执行次数
            if let Some(expires_at) = job.expires_at {
                if now >= expires_at {
                    self.complete_job(&job.id).await;
                    continue;
                }
            }

            if let Some(max_runs) = job.max_runs {
                if job.run_count >= max_runs {
                    self.complete_job(&job.id).await;
                    continue;
                }
            }

            // 标记为执行中，防止重复调度
            {
                let mut running = self.running.write().await;
                running.insert(job.id.clone());
            }

            // 从队列中移除（避免重复触发）
            self.remove_job(&job.id).await;

            // 发送任务到 worker
            if let Err(e) = self.task_tx.send(job.clone()).await {
                tracing::error!("Failed to send cron job to worker: {}", e);
                // 发送失败（worker 已关闭）：移出 running，延迟 60s 重新入队重试
                let mut running = self.running.write().await;
                running.remove(&job.id);
                drop(running);
                let retry_at = Utc::now() + chrono::Duration::seconds(60);
                let mut queue = self.queue.write().await;
                queue.entry(retry_at).or_default().push(job.id.clone());
            }
        }

        Ok(())
    }

    /// 将任务标记为完成，从队列和缓存中移除并更新数据库
    async fn complete_job(&self, job_id: &CronJobId) {
        self.remove_job(job_id).await;
        if let Err(e) = self
            .store
            .update(
                job_id,
                &crate::cron::types::UpdateCronJobInput {
                    status: Some(CronJobStatus::Completed),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::warn!("Failed to mark cron job {} as completed: {}", job_id.0, e);
        }
        let mut jobs = self.jobs.write().await;
        jobs.remove(job_id);
    }

    /// 从队列中移除指定任务（扫描所有 key，避免缓存的 `next_run_at`
    /// 与队列不一致时留下残留 entry）
    async fn remove_job(&self, job_id: &CronJobId) {
        let mut queue = self.queue.write().await;
        queue.retain(|_, ids| {
            ids.retain(|id| id != job_id);
            !ids.is_empty()
        });
    }
}

#[cfg(test)]
#[path = "scheduler_test.rs"]
mod tests;
