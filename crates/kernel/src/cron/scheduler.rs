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
    pub(crate) shutdown: CancellationToken,
}

impl CronScheduler {
    pub fn new(
        store: Arc<dyn CronStore>,
        task_tx: mpsc::Sender<CronJob>,
        shutdown: CancellationToken,
    ) -> Self {
        let (reload_tx, _reload_rx) = tokio::sync::watch::channel(0u64);
        Self {
            store,
            task_tx,
            queue: Arc::new(RwLock::new(BTreeMap::new())),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(HashSet::new())),
            reload_tx,
            shutdown,
        }
    }

    /// 启动调度主循环
    pub async fn run(self: Arc<Self>) {
        if let Err(e) = self.load_jobs().await {
            tracing::error!("Failed to load cron jobs: {e}");
        }

        let mut reload_rx = self.reload_tx.subscribe();

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
                () = self.shutdown.cancelled() => {
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
                        continue;
                    }
                    if let Some(job) = jobs.get(job_id) {
                        due_jobs.push(job.clone());
                    }
                }
            }
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
                // 发送失败，从 running 中移除
                let mut running = self.running.write().await;
                running.remove(&job.id);
                // TODO: 重新将任务入队
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

    /// 从队列中移除指定任务
    async fn remove_job(&self, job_id: &CronJobId) {
        let next_run = {
            let jobs = self.jobs.read().await;
            jobs.get(job_id).and_then(|j| j.next_run_at)
        };
        if let Some(t) = next_run {
            let mut queue = self.queue.write().await;
            if let Some(ids) = queue.get_mut(&t) {
                ids.retain(|id| id != job_id);
                if ids.is_empty() {
                    queue.remove(&t);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::store::CronStore;
    use crate::cron::types::{CronAction, CronJob, CronJobId, CronJobStatus, UpdateCronJobInput};
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MockStore {
        jobs: Mutex<HashMap<String, CronJob>>,
        updates: Mutex<Vec<(String, UpdateCronJobInput)>>,
    }

    impl MockStore {
        fn new(jobs: HashMap<String, CronJob>) -> Self {
            Self {
                jobs: Mutex::new(jobs),
                updates: Mutex::new(Vec::new()),
            }
        }

        fn updates(&self) -> Vec<(String, UpdateCronJobInput)> {
            self.updates.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CronStore for MockStore {
        async fn create(&self, _job: &CronJob) -> Result<(), CronError> {
            Ok(())
        }

        async fn get(&self, id: &CronJobId) -> Result<Option<CronJob>, CronError> {
            Ok(self.jobs.lock().unwrap().get(&id.0).cloned())
        }

        async fn list(
            &self,
            _status: Option<CronJobStatus>,
            _limit: usize,
        ) -> Result<Vec<CronJob>, CronError> {
            Ok(self.jobs.lock().unwrap().values().cloned().collect())
        }

        async fn update(
            &self,
            id: &CronJobId,
            input: &UpdateCronJobInput,
        ) -> Result<bool, CronError> {
            self.updates
                .lock()
                .unwrap()
                .push((id.0.clone(), input.clone()));
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(job) = jobs.get_mut(&id.0) {
                if let Some(name) = &input.name {
                    job.name.clone_from(name);
                }
                if let Some(schedule) = &input.schedule {
                    job.schedule.clone_from(schedule);
                }
                if let Some(action) = &input.action {
                    job.action = action.clone();
                }
                if let Some(status) = input.status {
                    job.status = status;
                }
                if let Some(max_runs) = input.max_runs {
                    job.max_runs = Some(max_runs);
                }
                if let Some(expires_at) = input.expires_at {
                    job.expires_at = Some(expires_at);
                }
                if let Some(next_run) = input.next_run_at {
                    job.next_run_at = Some(next_run);
                }
            }
            Ok(true)
        }

        async fn delete(&self, _id: &CronJobId) -> Result<bool, CronError> {
            Ok(true)
        }

        async fn list_active(&self) -> Result<Vec<CronJob>, CronError> {
            Ok(self
                .jobs
                .lock()
                .unwrap()
                .values()
                .filter(|j| matches!(j.status, CronJobStatus::Active))
                .cloned()
                .collect())
        }

        async fn record_execution(
            &self,
            _id: &CronJobId,
            _error: Option<String>,
        ) -> Result<(), CronError> {
            Ok(())
        }
    }

    fn make_job(
        id: &str,
        schedule: &str,
        next_run: Option<DateTime<Utc>>,
        expires_at: Option<DateTime<Utc>>,
        max_runs: Option<u32>,
        run_count: u32,
    ) -> CronJob {
        CronJob {
            id: CronJobId(id.to_string()),
            name: "test".to_string(),
            schedule: schedule.to_string(),
            action: CronAction::Shell {
                command: "echo hi".to_string(),
                working_dir: None,
            },
            status: CronJobStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            next_run_at: next_run,
            last_run_at: None,
            run_count,
            max_runs,
            expires_at,
            last_error: None,
        }
    }

    #[tokio::test]
    async fn test_fire_due_jobs_triggers_expired() {
        let (tx, mut rx) = mpsc::channel(10);
        let past = Utc::now() - chrono::Duration::seconds(60);
        let job = make_job("j1", "0 0 9 * * *", Some(past), None, None, 0);

        let mut jobs = HashMap::new();
        jobs.insert("j1".to_string(), job.clone());
        let store = Arc::new(MockStore::new(jobs));
        let scheduler = Arc::new(CronScheduler::new(store, tx, CancellationToken::new()));

        {
            let mut q = scheduler.queue.write().await;
            q.entry(past).or_default().push(job.id.clone());
            let mut j = scheduler.jobs.write().await;
            j.insert(job.id.clone(), job.clone());
        }

        scheduler.fire_due_jobs().await.unwrap();

        let received = rx.try_recv().unwrap();
        assert_eq!(received.id.0, "j1");

        let running = scheduler.running.read().await;
        assert!(running.contains(&job.id));

        let q = scheduler.queue.read().await;
        assert!(q.is_empty());
    }

    #[tokio::test]
    async fn test_fire_due_jobs_skips_future() {
        let (tx, mut rx) = mpsc::channel(10);
        let future = Utc::now() + chrono::Duration::seconds(3600);
        let job = make_job("j1", "0 0 9 * * *", Some(future), None, None, 0);

        let mut jobs = HashMap::new();
        jobs.insert("j1".to_string(), job.clone());
        let store = Arc::new(MockStore::new(jobs));
        let scheduler = Arc::new(CronScheduler::new(store, tx, CancellationToken::new()));

        {
            let mut q = scheduler.queue.write().await;
            q.entry(future).or_default().push(job.id.clone());
            let mut j = scheduler.jobs.write().await;
            j.insert(job.id.clone(), job.clone());
        }

        scheduler.fire_due_jobs().await.unwrap();

        assert!(rx.try_recv().is_err());

        let q = scheduler.queue.read().await;
        assert_eq!(q.len(), 1);
    }

    #[tokio::test]
    async fn test_fire_due_jobs_skips_running() {
        let (tx, mut rx) = mpsc::channel(10);
        let past = Utc::now() - chrono::Duration::seconds(60);
        let job = make_job("j1", "0 0 9 * * *", Some(past), None, None, 0);

        let mut jobs = HashMap::new();
        jobs.insert("j1".to_string(), job.clone());
        let store = Arc::new(MockStore::new(jobs));
        let scheduler = Arc::new(CronScheduler::new(store, tx, CancellationToken::new()));

        {
            let mut q = scheduler.queue.write().await;
            q.entry(past).or_default().push(job.id.clone());
            let mut j = scheduler.jobs.write().await;
            j.insert(job.id.clone(), job.clone());
            let mut r = scheduler.running.write().await;
            r.insert(job.id.clone());
        }

        scheduler.fire_due_jobs().await.unwrap();

        assert!(rx.try_recv().is_err());

        let q = scheduler.queue.read().await;
        assert_eq!(q.len(), 1);
    }

    #[tokio::test]
    async fn test_fire_due_jobs_completes_expired_job() {
        let (tx, mut rx) = mpsc::channel(10);
        let past = Utc::now() - chrono::Duration::seconds(60);
        let expired = Utc::now() - chrono::Duration::seconds(30);
        let job = make_job("j1", "0 0 9 * * *", Some(past), Some(expired), None, 0);

        let mut jobs = HashMap::new();
        jobs.insert("j1".to_string(), job.clone());
        let store = Arc::new(MockStore::new(jobs));
        let scheduler = Arc::new(CronScheduler::new(
            store.clone(),
            tx,
            CancellationToken::new(),
        ));

        {
            let mut q = scheduler.queue.write().await;
            q.entry(past).or_default().push(job.id.clone());
            let mut j = scheduler.jobs.write().await;
            j.insert(job.id.clone(), job.clone());
        }

        scheduler.fire_due_jobs().await.unwrap();

        assert!(rx.try_recv().is_err());

        let q = scheduler.queue.read().await;
        assert!(q.is_empty());

        let updates = store.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, "j1");
        assert_eq!(updates[0].1.status, Some(CronJobStatus::Completed));
    }

    #[tokio::test]
    async fn test_fire_due_jobs_completes_max_runs() {
        let (tx, mut rx) = mpsc::channel(10);
        let past = Utc::now() - chrono::Duration::seconds(60);
        let job = make_job("j1", "0 0 9 * * *", Some(past), None, Some(5), 5);

        let mut jobs = HashMap::new();
        jobs.insert("j1".to_string(), job.clone());
        let store = Arc::new(MockStore::new(jobs));
        let scheduler = Arc::new(CronScheduler::new(
            store.clone(),
            tx,
            CancellationToken::new(),
        ));

        {
            let mut q = scheduler.queue.write().await;
            q.entry(past).or_default().push(job.id.clone());
            let mut j = scheduler.jobs.write().await;
            j.insert(job.id.clone(), job.clone());
        }

        scheduler.fire_due_jobs().await.unwrap();

        assert!(rx.try_recv().is_err());

        let q = scheduler.queue.read().await;
        assert!(q.is_empty());

        let updates = store.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, "j1");
        assert_eq!(updates[0].1.status, Some(CronJobStatus::Completed));
    }

    #[tokio::test]
    async fn test_remove_job_single_timestamp() {
        let (tx, _rx) = mpsc::channel(10);
        let past = Utc::now() - chrono::Duration::seconds(60);
        let job = make_job("j1", "0 0 9 * * *", Some(past), None, None, 0);

        let store = Arc::new(MockStore::new(HashMap::new()));
        let scheduler = Arc::new(CronScheduler::new(store, tx, CancellationToken::new()));

        {
            let mut q = scheduler.queue.write().await;
            q.entry(past).or_default().push(job.id.clone());
            let mut j = scheduler.jobs.write().await;
            j.insert(job.id.clone(), job.clone());
        }

        scheduler.remove_job(&job.id).await;

        let q = scheduler.queue.read().await;
        assert!(q.is_empty());
    }

    #[tokio::test]
    async fn test_remove_job_shared_timestamp() {
        let (tx, _rx) = mpsc::channel(10);
        let past = Utc::now() - chrono::Duration::seconds(60);
        let job1 = make_job("j1", "0 0 9 * * *", Some(past), None, None, 0);
        let job2 = make_job("j2", "0 0 9 * * *", Some(past), None, None, 0);

        let mut jobs = HashMap::new();
        jobs.insert("j1".to_string(), job1.clone());
        jobs.insert("j2".to_string(), job2.clone());
        let store = Arc::new(MockStore::new(jobs));
        let scheduler = Arc::new(CronScheduler::new(store, tx, CancellationToken::new()));

        {
            let mut q = scheduler.queue.write().await;
            q.entry(past).or_default().push(job1.id.clone());
            q.entry(past).or_default().push(job2.id.clone());
            let mut j = scheduler.jobs.write().await;
            j.insert(job1.id.clone(), job1.clone());
            j.insert(job2.id.clone(), job2.clone());
        }

        scheduler.remove_job(&job1.id).await;

        let q = scheduler.queue.read().await;
        assert_eq!(q.len(), 1);
        let ids = q.get(&past).unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].0, "j2");
    }

    #[tokio::test]
    async fn test_job_finished_requeues() {
        let (tx, _rx) = mpsc::channel(10);
        let past = Utc::now() - chrono::Duration::seconds(60);
        let job = make_job("j1", "0 0 9 * * *", Some(past), None, None, 0);

        let mut jobs = HashMap::new();
        jobs.insert("j1".to_string(), job.clone());
        let store = Arc::new(MockStore::new(jobs));
        let scheduler = Arc::new(CronScheduler::new(
            store.clone(),
            tx,
            CancellationToken::new(),
        ));

        {
            let mut q = scheduler.queue.write().await;
            q.entry(past).or_default().push(job.id.clone());
            let mut j = scheduler.jobs.write().await;
            j.insert(job.id.clone(), job.clone());
            let mut r = scheduler.running.write().await;
            r.insert(job.id.clone());
        }

        scheduler.job_finished(&job.id).await;

        let running = scheduler.running.read().await;
        assert!(!running.contains(&job.id));

        let q = scheduler.queue.read().await;
        assert_eq!(q.len(), 1);
        // next_run 应该是明天 9:00，在 future
        let (next_run, ids) = q.first_key_value().unwrap();
        assert!(*next_run > Utc::now());
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].0, "j1");

        let updates = store.updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, "j1");
        assert!(updates[0].1.next_run_at.is_some());
    }
}
