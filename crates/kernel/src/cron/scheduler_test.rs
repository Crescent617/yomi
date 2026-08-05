use super::*;

use crate::cron::store::CronStore;
use crate::cron::types::{
    CronAction, CronJob, CronJobId, CronJobStatus, UpdateCronJobInput, NEVER_EXPIRES,
    UNLIMITED_MAX_RUNS,
};
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
        Ok(self.jobs.lock().unwrap().get(id.0.as_str()).cloned())
    }

    async fn list(
        &self,
        _status: Option<CronJobStatus>,
        _limit: usize,
    ) -> Result<Vec<CronJob>, CronError> {
        Ok(self.jobs.lock().unwrap().values().cloned().collect())
    }

    async fn update(&self, id: &CronJobId, input: &UpdateCronJobInput) -> Result<bool, CronError> {
        self.updates
            .lock()
            .unwrap()
            .push((id.0.to_string(), input.clone()));
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(id.0.as_str()) {
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
                job.max_runs = max_runs;
            }
            if let Some(expires_at) = input.expires_at {
                job.expires_at = expires_at;
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
        id: CronJobId::from(id.to_string()),
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
        max_runs: max_runs.unwrap_or(UNLIMITED_MAX_RUNS),
        last_error: None,
        expires_at: expires_at.unwrap_or(NEVER_EXPIRES),
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
    let scheduler = Arc::new(CronScheduler::new(store, tx));

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
    let scheduler = Arc::new(CronScheduler::new(store, tx));

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
    let scheduler = Arc::new(CronScheduler::new(store, tx));

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

    // 执行中的 job 的过期 entry 会被丢弃（避免调度循环空转），
    // 执行结束后由 job_finished 重新入队
    let q = scheduler.queue.read().await;
    assert!(q.is_empty());
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
    let scheduler = Arc::new(CronScheduler::new(store.clone(), tx));

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
    let scheduler = Arc::new(CronScheduler::new(store.clone(), tx));

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
async fn test_fire_due_jobs_zero_sentinels_never_complete() {
    // max_runs = 0 (unlimited) and expires_at = zero timestamp (never): even
    // with run_count far beyond any limit and the sentinel date long past,
    // the job must still fire, never complete.
    let (tx, mut rx) = mpsc::channel(10);
    let past = Utc::now() - chrono::Duration::seconds(60);
    let job = make_job(
        "j1",
        "0 0 9 * * *",
        Some(past),
        Some(NEVER_EXPIRES),
        Some(UNLIMITED_MAX_RUNS),
        999,
    );

    let mut jobs = HashMap::new();
    jobs.insert("j1".to_string(), job.clone());
    let store = Arc::new(MockStore::new(jobs));
    let scheduler = Arc::new(CronScheduler::new(store.clone(), tx));

    {
        let mut q = scheduler.queue.write().await;
        q.entry(past).or_default().push(job.id.clone());
        let mut j = scheduler.jobs.write().await;
        j.insert(job.id.clone(), job.clone());
    }

    scheduler.fire_due_jobs().await.unwrap();

    // Fired, not completed.
    let received = rx.try_recv().unwrap();
    assert_eq!(received.id.0, "j1");
    assert!(
        store
            .updates()
            .iter()
            .all(|(_, input)| input.status != Some(CronJobStatus::Completed)),
        "sentinel-limited job must never be completed"
    );
}

#[tokio::test]
async fn test_remove_job_single_timestamp() {
    let (tx, _rx) = mpsc::channel(10);
    let past = Utc::now() - chrono::Duration::seconds(60);
    let job = make_job("j1", "0 0 9 * * *", Some(past), None, None, 0);

    let store = Arc::new(MockStore::new(HashMap::new()));
    let scheduler = Arc::new(CronScheduler::new(store, tx));

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
    let scheduler = Arc::new(CronScheduler::new(store, tx));

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
    let scheduler = Arc::new(CronScheduler::new(store.clone(), tx));

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

#[tokio::test]
async fn test_stale_removal_preserves_concurrent_requeue() {
    let (tx, mut rx) = mpsc::channel(10);
    let past = Utc::now() - chrono::Duration::seconds(60);
    let future = Utc::now() + chrono::Duration::seconds(60);
    let job = make_job("j1", "0 0 9 * * *", Some(future), None, None, 0);

    let mut jobs = HashMap::new();
    jobs.insert("j1".to_string(), job.clone());
    let store = Arc::new(MockStore::new(jobs));
    let scheduler = Arc::new(CronScheduler::new(store, tx));

    // The job is running; a stale due entry exists (reload re-queued it),
    // and job_finished has already pushed a fresh entry at a later time.
    {
        let mut running = scheduler.running.write().await;
        running.insert(job.id.clone());
        let mut q = scheduler.queue.write().await;
        q.entry(past).or_default().push(job.id.clone());
        q.entry(future).or_default().push(job.id.clone());
        let mut j = scheduler.jobs.write().await;
        j.insert(job.id.clone(), job.clone());
    }

    scheduler.fire_due_jobs().await.unwrap();

    // The stale entry is dropped, but the fresh requeue at the different
    // timestamp survives the scoped removal; nothing fired.
    let q = scheduler.queue.read().await;
    assert!(q.get(&past).is_none(), "stale entry removed");
    assert_eq!(
        q.get(&future).map(|ids| ids.len()),
        Some(1),
        "fresh requeue preserved"
    );
    assert!(rx.try_recv().is_err(), "nothing fired");
}

#[tokio::test]
async fn job_finished_wakes_scheduler_loop() {
    // Regression: a requeued job must wake the run loop. Without the notify,
    // the loop parks on an empty queue (sleep branch disabled) and recurring
    // jobs stall after their first fire until an unrelated reload.
    let job = make_job("j1", "0 0 9 * * *", None, None, None, 0);
    let mut jobs = HashMap::new();
    jobs.insert(job.id.0.to_string(), job.clone());
    let store = Arc::new(MockStore::new(jobs));
    let (tx, _rx) = mpsc::channel(1);
    let scheduler = CronScheduler::new(store, tx);

    scheduler.job_finished(&job.id).await;

    tokio::time::timeout(
        std::time::Duration::from_millis(50),
        scheduler.queue_wake.notified(),
    )
    .await
    .expect("job_finished must wake the scheduler loop");
}
