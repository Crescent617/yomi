use super::*;
use crate::cron::types::{
    CronAction, CronJobId, CronJobStatus, UpdateCronJobInput, NEVER_EXPIRES, UNLIMITED_MAX_RUNS,
};
use crate::cron::{CronActionOutcome, CronScheduler};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Mutex;

/// 只跟踪 finalize 关心的两个副作用：执行记录与状态更新。
#[derive(Default)]
struct MockStore {
    jobs: Mutex<HashMap<String, CronJob>>,
    records: Mutex<Vec<Option<String>>>,
    updates: Mutex<Vec<UpdateCronJobInput>>,
}

#[async_trait::async_trait]
impl CronStore for MockStore {
    async fn create(&self, job: &CronJob) -> Result<(), CronError> {
        self.jobs
            .lock()
            .unwrap()
            .insert(job.id.0.to_string(), job.clone());
        Ok(())
    }

    async fn get(&self, id: &CronJobId) -> Result<Option<CronJob>, CronError> {
        Ok(self.jobs.lock().unwrap().get(id.0.as_str()).cloned())
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<CronJob>, CronError> {
        Ok(self
            .jobs
            .lock()
            .unwrap()
            .values()
            .find(|j| j.name == name)
            .cloned())
    }

    async fn list(
        &self,
        _status: Option<CronJobStatus>,
        _limit: usize,
    ) -> Result<Vec<CronJob>, CronError> {
        Ok(self.jobs.lock().unwrap().values().cloned().collect())
    }

    async fn update(&self, id: &CronJobId, input: &UpdateCronJobInput) -> Result<bool, CronError> {
        self.updates.lock().unwrap().push(input.clone());
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id.0.as_str()) {
            if let Some(status) = &input.status {
                job.status = *status;
            }
        }
        Ok(true)
    }

    async fn delete(&self, id: &CronJobId) -> Result<bool, CronError> {
        Ok(self.jobs.lock().unwrap().remove(id.0.as_str()).is_some())
    }

    async fn list_active(&self) -> Result<Vec<CronJob>, CronError> {
        Ok(vec![])
    }

    async fn record_execution(
        &self,
        id: &CronJobId,
        error: Option<String>,
    ) -> Result<(), CronError> {
        self.records.lock().unwrap().push(error);
        if let Some(job) = self.jobs.lock().unwrap().get_mut(id.0.as_str()) {
            job.run_count += 1;
        }
        Ok(())
    }
}

impl MockStore {
    fn records(&self) -> Vec<Option<String>> {
        self.records.lock().unwrap().clone()
    }

    fn updates(&self) -> Vec<UpdateCronJobInput> {
        self.updates.lock().unwrap().clone()
    }
}

fn make_job(id: &str) -> CronJob {
    CronJob {
        id: CronJobId::from(id.to_string()),
        name: "test".to_string(),
        schedule: "0 0 9 * * *".to_string(),
        action: CronAction::Shell {
            command: "true".to_string(),
            working_dir: None,
        },
        status: CronJobStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        next_run_at: Some(Utc::now() + chrono::Duration::hours(1)),
        last_run_at: None,
        run_count: 0,
        max_runs: UNLIMITED_MAX_RUNS,
        expires_at: NEVER_EXPIRES,
        last_error: None,
    }
}

async fn fixture() -> (Arc<MockStore>, CronJob, Arc<CronScheduler>) {
    let store = Arc::new(MockStore::default());
    let job = make_job("j1");
    store.create(&job).await.unwrap();
    let (tx, _rx) = mpsc::channel(1);
    let scheduler = Arc::new(CronScheduler::new(store.clone(), tx));
    (store, job, scheduler)
}

#[tokio::test]
async fn finalize_done_records_success_and_keeps_job_active() {
    let (store, job, scheduler) = fixture().await;

    CronWorker::finalize(
        store.clone(),
        Some(scheduler),
        &job,
        Ok(CronActionOutcome::Done),
    )
    .await;

    assert_eq!(store.records(), &[None]);
    assert!(
        store.updates().iter().all(|u| u.status.is_none()),
        "Done must not touch job status"
    );
}

#[tokio::test]
async fn finalize_self_complete_marks_completed() {
    let (store, job, scheduler) = fixture().await;

    CronWorker::finalize(
        store.clone(),
        Some(scheduler),
        &job,
        Ok(CronActionOutcome::SelfComplete),
    )
    .await;

    assert_eq!(store.records(), &[None], "self-complete is not an error");
    let updates = store.updates();
    assert!(
        matches!(
            updates.first().and_then(|u| u.status),
            Some(CronJobStatus::Completed)
        ),
        "SelfComplete must mark the job Completed, got {updates:?}"
    );
}

#[tokio::test]
async fn finalize_error_records_failure_and_keeps_job_active() {
    let (store, job, scheduler) = fixture().await;

    CronWorker::finalize(
        store.clone(),
        Some(scheduler),
        &job,
        Err(CronError::ShellFailed("boom".to_string())),
    )
    .await;

    let records = store.records();
    assert!(
        matches!(records.first(), Some(Some(e)) if e.contains("boom")),
        "failure must be recorded, got {records:?}"
    );
    assert!(
        store.updates().iter().all(|u| u.status.is_none()),
        "failure must not complete the job"
    );
}
