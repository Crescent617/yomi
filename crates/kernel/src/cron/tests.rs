#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::types::{
        CronAction, CronError, CronJob, CronJobId, CronJobStatus, CronSchedule, NEVER_EXPIRES,
    };
    use chrono::{Timelike, Utc};

    #[test]
    fn test_cron_schedule_parse_valid() {
        let schedule = CronSchedule::parse("0 0 9 * * *");
        assert!(schedule.is_ok());
    }

    #[test]
    fn test_cron_schedule_parse_invalid() {
        let schedule = CronSchedule::parse("invalid");
        assert!(matches!(schedule, Err(CronError::InvalidSchedule(_))));
    }

    #[test]
    fn test_cron_schedule_next_after() {
        let schedule = CronSchedule::parse("0 0 9 * * *").unwrap();
        let now = Utc::now();
        let next = schedule.next_after(now);
        assert!(next.is_some());
        let next = next.unwrap();
        // 表达式按本地时区解释：本地时间应为 9:00 AM
        let local = next.with_timezone(&chrono::Local);
        assert_eq!(local.hour(), 9);
        assert_eq!(local.minute(), 0);
        assert_eq!(local.second(), 0);
    }

    #[test]
    fn test_cron_schedule_upcoming() {
        let schedule = CronSchedule::parse("0 0 9 * * *").unwrap();
        let now = Utc::now();
        let upcoming = schedule.upcoming(now, 3);
        assert_eq!(upcoming.len(), 3);
        for t in &upcoming {
            let local = t.with_timezone(&chrono::Local);
            assert_eq!(local.hour(), 9);
            assert_eq!(local.minute(), 0);
            assert_eq!(local.second(), 0);
        }
    }

    #[test]
    fn test_cron_job_status_from_str() {
        assert_eq!(
            "active".parse::<CronJobStatus>().unwrap(),
            CronJobStatus::Active
        );
        assert_eq!(
            "paused".parse::<CronJobStatus>().unwrap(),
            CronJobStatus::Paused
        );
        assert_eq!(
            "completed".parse::<CronJobStatus>().unwrap(),
            CronJobStatus::Completed
        );
        assert_eq!(
            "failed".parse::<CronJobStatus>().unwrap(),
            CronJobStatus::Failed
        );
        assert!("invalid".parse::<CronJobStatus>().is_err());
    }

    #[test]
    fn test_cron_job_status_as_str() {
        assert_eq!(CronJobStatus::Active.as_str(), "active");
        assert_eq!(CronJobStatus::Paused.as_str(), "paused");
        assert_eq!(CronJobStatus::Completed.as_str(), "completed");
        assert_eq!(CronJobStatus::Failed.as_str(), "failed");
    }

    #[test]
    fn test_cron_job_id_new() {
        let id1 = CronJobId::new();
        let id2 = CronJobId::new();
        assert_ne!(id1.0, id2.0);
    }

    #[test]
    fn test_cron_action_serde() {
        let action = CronAction::SendMessage {
            session_id: Some("test-session".to_string()),
            content: "Hello {{date}}".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("send_message"));
        assert!(json.contains("test-session"));

        let decoded: CronAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, decoded);
    }

    #[test]
    fn test_cron_job_serde() {
        let job = CronJob {
            id: CronJobId::new(),
            name: "Test Job".to_string(),
            schedule: "0 0 9 * * *".to_string(),
            action: CronAction::Shell {
                command: "echo hello".to_string(),
                working_dir: None,
            },
            status: CronJobStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            next_run_at: None,
            last_run_at: None,
            run_count: 0,
            max_runs: 10,
            expires_at: NEVER_EXPIRES,
            last_error: None,
        };

        let json = serde_json::to_string(&job).unwrap();
        let decoded: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(job.name, decoded.name);
        assert_eq!(job.schedule, decoded.schedule);
        assert_eq!(job.status, decoded.status);
    }

    // The exit code is a public contract — external scripts hardcode it.
    #[test]
    fn self_complete_exit_code_is_42() {
        assert_eq!(super::super::SHELL_COMPLETE_EXIT_CODE, 42);
    }

    #[tokio::test]
    async fn shell_runner_success_captures_stdout() {
        let out = super::super::run_shell_command("echo hello", None)
            .await
            .unwrap();
        assert_eq!(out.stdout.trim(), "hello");
        assert!(!out.self_complete);
    }

    #[tokio::test]
    async fn shell_runner_complete_exit_code_marks_self_complete() {
        let cmd = format!("echo done; exit {}", super::super::SHELL_COMPLETE_EXIT_CODE);
        let out = super::super::run_shell_command(&cmd, None).await.unwrap();
        assert_eq!(out.stdout.trim(), "done");
        assert!(out.self_complete);
    }

    #[tokio::test]
    async fn shell_runner_other_nonzero_is_shell_failed() {
        let err = super::super::run_shell_command("echo boom >&2; exit 1", None)
            .await
            .unwrap_err();
        assert!(matches!(err, CronError::ShellFailed(e) if e.contains("boom")));
    }

    #[tokio::test]
    async fn shell_runner_signal_death_is_failure_not_self_complete() {
        // Killed by a signal → no exit code → failure, never self-complete.
        let err = super::super::run_shell_command("kill -9 $$", None)
            .await
            .unwrap_err();
        assert!(matches!(err, CronError::ShellFailed(_)));
    }

    // ── create_cron_job ensure 语义（按 name 幂等）─────────────────────

    async fn test_cron_store() -> std::sync::Arc<dyn super::super::CronStore> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::storage::migrations::run_migrations(&pool)
            .await
            .unwrap();
        std::sync::Arc::new(super::super::SqliteCronStore::new(pool))
    }

    fn shell_input(name: &str, schedule: &str) -> super::super::CreateCronJobInput {
        super::super::CreateCronJobInput {
            name: name.to_string(),
            schedule: schedule.to_string(),
            action: CronAction::Shell {
                command: "true".to_string(),
                working_dir: None,
            },
            max_runs: None,
            expires_at: None,
        }
    }

    #[tokio::test]
    async fn create_cron_job_returns_existing_on_name_conflict() {
        let store = test_cron_store().await;

        let first =
            super::super::create_cron_job(&store, None, None, shell_input("janitor", "0 9 * * *"))
                .await
                .unwrap();
        assert!(first.created);

        // 同名再 create：返回既有 job（同 id），新 schedule 不生效、不产生新行
        let second =
            super::super::create_cron_job(&store, None, None, shell_input("janitor", "0 10 * * *"))
                .await
                .unwrap();
        assert!(!second.created);
        assert_eq!(second.job.id.0, first.job.id.0);
        assert_eq!(second.job.schedule, "0 9 * * *");
        assert_eq!(store.list(None, 10).await.unwrap().len(), 1);

        // 不同名：正常新建
        let third =
            super::super::create_cron_job(&store, None, None, shell_input("other", "0 9 * * *"))
                .await
                .unwrap();
        assert!(third.created);
        assert_ne!(third.job.id.0, first.job.id.0);
    }

    // ── 并发撞名回退（唯一索引兜底路径）─────────────────────────────────

    /// 故障注入 store：首次 create 时模拟"竞态胜者抢先落库同名 job"，
    /// 并返回 DuplicateName——复现 pre-check 与 insert 之间的并发窗口。
    struct RaceStore {
        inner: super::super::SqliteCronStore,
        fired: std::sync::atomic::AtomicBool,
    }

    impl RaceStore {
        async fn new() -> Self {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            crate::storage::migrations::run_migrations(&pool)
                .await
                .unwrap();
            Self {
                inner: super::super::SqliteCronStore::new(pool),
                fired: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    #[async_trait::async_trait]
    impl super::super::CronStore for RaceStore {
        async fn create(&self, job: &CronJob) -> Result<(), CronError> {
            use std::sync::atomic::Ordering;
            if !self.fired.swap(true, Ordering::SeqCst) {
                // 竞态胜者抢先落库同名 job（不同 id），我们这边 insert 撞唯一索引
                let mut winner = job.clone();
                winner.id = CronJobId::from("cron-winner");
                self.inner.create(&winner).await?;
                return Err(CronError::DuplicateName(job.name.clone()));
            }
            self.inner.create(job).await
        }

        async fn get(&self, id: &CronJobId) -> Result<Option<CronJob>, CronError> {
            self.inner.get(id).await
        }

        async fn get_by_name(&self, name: &str) -> Result<Option<CronJob>, CronError> {
            self.inner.get_by_name(name).await
        }

        async fn list(
            &self,
            status: Option<CronJobStatus>,
            limit: usize,
        ) -> Result<Vec<CronJob>, CronError> {
            self.inner.list(status, limit).await
        }

        async fn update(
            &self,
            id: &CronJobId,
            input: &super::super::UpdateCronJobInput,
        ) -> Result<bool, CronError> {
            self.inner.update(id, input).await
        }

        async fn delete(&self, id: &CronJobId) -> Result<bool, CronError> {
            self.inner.delete(id).await
        }

        async fn list_active(&self) -> Result<Vec<CronJob>, CronError> {
            self.inner.list_active().await
        }

        async fn record_execution(
            &self,
            id: &CronJobId,
            error: Option<String>,
        ) -> Result<(), CronError> {
            self.inner.record_execution(id, error).await
        }
    }

    #[tokio::test]
    async fn create_cron_job_recovers_from_concurrent_name_race() {
        let store: std::sync::Arc<dyn super::super::CronStore> =
            std::sync::Arc::new(RaceStore::new().await);

        // pre-check 时库里还没有，insert 时撞名——回退应返回竞态胜者而非报错
        let out =
            super::super::create_cron_job(&store, None, None, shell_input("janitor", "0 9 * * *"))
                .await
                .unwrap();

        assert!(!out.created);
        assert_eq!(out.job.id.0, "cron-winner");
        assert_eq!(store.list(None, 10).await.unwrap().len(), 1);
    }
}
