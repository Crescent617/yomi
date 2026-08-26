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
    fn test_cron_schedule_day_of_week_convention() {
        // 星期字段采用 UNIX/Vixie 约定：0 或 7=周日，1=周一 … 6=周六；
        // 英文缩写同样可用。周一至周五 = 1-5。
        use chrono::{Datelike, TimeZone, Weekday};

        // 锚点：2026-08-14 周五 10:00（本地）
        let from = chrono::Local
            .with_ymd_and_hms(2026, 8, 14, 10, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let next = |expr: &str| {
            CronSchedule::parse(expr)
                .unwrap()
                .next_after(from)
                .unwrap()
                .with_timezone(&chrono::Local)
        };

        // 0 与 7 都是周日 → 8-16
        let t = next("0 0 9 * * 0");
        assert_eq!((t.weekday(), t.day()), (Weekday::Sun, 16));
        let t = next("0 0 9 * * 7");
        assert_eq!((t.weekday(), t.day()), (Weekday::Sun, 16));

        // 1=周一 → 8-17；6=周六 → 8-15
        let t = next("0 0 9 * * 1");
        assert_eq!((t.weekday(), t.day()), (Weekday::Mon, 17));
        let t = next("0 0 9 * * 6");
        assert_eq!((t.weekday(), t.day()), (Weekday::Sat, 15));

        // 1-5 = 周一至周五 → 8-17（周一）
        let t = next("0 0 9 * * 1-5");
        assert_eq!((t.weekday(), t.day()), (Weekday::Mon, 17));
        // 2-6 = 周二至周六 → 8-15（周六）
        let t = next("0 0 9 * * 2-6");
        assert_eq!((t.weekday(), t.day()), (Weekday::Sat, 15));

        // 跨界区间 5-7 = 五六日 → 8-15（周六）
        let t = next("0 0 9 * * 5-7");
        assert_eq!((t.weekday(), t.day()), (Weekday::Sat, 15));
        // 步进 */2 = UNIX {日,二,四,六} → 8-15（周六）
        let t = next("0 0 9 * * */2");
        assert_eq!((t.weekday(), t.day()), (Weekday::Sat, 15));

        // 英文缩写不变：fri → 8-21（周五）
        let t = next("0 0 9 * * fri");
        assert_eq!((t.weekday(), t.day()), (Weekday::Fri, 21));

        // 越界报错
        assert!(CronSchedule::parse("0 0 9 * * 8").is_err());
        assert!(CronSchedule::parse("0 0 9 * * 1/0").is_err());
    }

    #[test]
    fn test_cron_schedule_seconds_optional() {
        // 秒字段可选：5 段秒省略为 0，6 段首字段为秒——位置语义与旧版一致，
        // 无需任何兼容/迁移处理。
        use chrono::{TimeZone, Timelike};

        let from = chrono::Local
            .with_ymd_and_hms(2026, 8, 14, 0, 0, 0)
            .unwrap()
            .with_timezone(&Utc);

        // 5 段与 6 段（显式 0 秒）等价：下一次都是本地 09:00:00
        for expr in ["0 9 * * *", "0 0 9 * * *"] {
            let next = CronSchedule::parse(expr)
                .unwrap()
                .next_after(from)
                .unwrap()
                .with_timezone(&chrono::Local);
            assert_eq!(
                (next.hour(), next.minute(), next.second()),
                (9, 0, 0),
                "{expr}"
            );
        }

        // 显式秒：09:00:30
        let next = CronSchedule::parse("30 0 9 * * *")
            .unwrap()
            .next_after(from)
            .unwrap()
            .with_timezone(&chrono::Local);
        assert_eq!((next.hour(), next.minute(), next.second()), (9, 0, 30));
    }

    #[test]
    fn test_cron_schedule_next_after_is_strictly_after() {
        // 不变量：next_after 严格晚于 from。DST 秋拨的重叠小时里 croner
        // 会把歧义 wall time 解析到绝对时间的"过去"（earliest），
        // `dt > from` 过滤是防热重跑的承重墙。
        use chrono::TimeZone;
        let from = chrono::Local
            .with_ymd_and_hms(2026, 8, 14, 10, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        for expr in ["* * * * * *", "0 9 * * *", "0 0 9 * * 1-5", "30 0 9 * * *"] {
            let next = CronSchedule::parse(expr).unwrap().next_after(from).unwrap();
            assert!(next > from, "{expr} -> {next} !> {from}");
        }
        // upcoming 同样不含"过去"
        let ups = CronSchedule::parse("* * * * * *")
            .unwrap()
            .upcoming(from, 3);
        assert_eq!(ups.len(), 3);
        assert!(ups.iter().all(|dt| *dt > from));
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
        let out = super::super::run_shell_command("echo hello", None, std::path::Path::new("/d"))
            .await
            .unwrap();
        assert_eq!(out.stdout.trim(), "hello");
        assert!(!out.self_complete);
    }

    #[tokio::test]
    async fn shell_runner_injects_yomi_data_dir() {
        let out = super::super::run_shell_command(
            "echo \"dir=$YOMI_DATA_DIR\"; echo \"sid=$YOMI_SESSION_ID\"",
            None,
            std::path::Path::new("/d"),
        )
        .await
        .unwrap();
        // cron shell job 无会话：YOMI_SESSION_ID 被显式移除而非继承父进程
        assert_eq!(out.stdout, "dir=/d\nsid=\n");
    }

    #[tokio::test]
    async fn shell_runner_complete_exit_code_marks_self_complete() {
        let cmd = format!("echo done; exit {}", super::super::SHELL_COMPLETE_EXIT_CODE);
        let out = super::super::run_shell_command(&cmd, None, std::path::Path::new("/d"))
            .await
            .unwrap();
        assert_eq!(out.stdout.trim(), "done");
        assert!(out.self_complete);
    }

    #[tokio::test]
    async fn shell_runner_other_nonzero_is_shell_failed() {
        let err = super::super::run_shell_command(
            "echo boom >&2; exit 1",
            None,
            std::path::Path::new("/d"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CronError::ShellFailed(e) if e.contains("boom")));
    }

    #[tokio::test]
    async fn shell_runner_signal_death_is_failure_not_self_complete() {
        // Killed by a signal → no exit code → failure, never self-complete.
        let err = super::super::run_shell_command("kill -9 $$", None, std::path::Path::new("/d"))
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

        let first = super::super::create_cron_job(
            &store,
            None,
            None,
            shell_input("janitor", "0 9 * * *"),
            crate::permission::Level::Safe,
        )
        .await
        .unwrap();
        assert!(first.created);

        // 同名再 create：返回既有 job（同 id），新 schedule 不生效、不产生新行
        let second = super::super::create_cron_job(
            &store,
            None,
            None,
            shell_input("janitor", "0 10 * * *"),
            crate::permission::Level::Safe,
        )
        .await
        .unwrap();
        assert!(!second.created);
        assert_eq!(second.job.id.0, first.job.id.0);
        assert_eq!(second.job.schedule, "0 9 * * *");
        assert_eq!(store.list(None, 10).await.unwrap().len(), 1);

        // 不同名：正常新建
        let third = super::super::create_cron_job(
            &store,
            None,
            None,
            shell_input("other", "0 9 * * *"),
            crate::permission::Level::Safe,
        )
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
        let out = super::super::create_cron_job(
            &store,
            None,
            None,
            shell_input("janitor", "0 9 * * *"),
            crate::permission::Level::Safe,
        )
        .await
        .unwrap();

        assert!(!out.created);
        assert_eq!(out.job.id.0, "cron-winner");
        assert_eq!(store.list(None, 10).await.unwrap().len(), 1);
    }

    // ── ensure_action_session 权限等级：follow config，下限 caution ─────

    async fn test_session_store() -> std::sync::Arc<dyn crate::storage::SessionStore> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::storage::migrations::run_migrations(&pool)
            .await
            .unwrap();
        std::sync::Arc::new(crate::storage::SqliteSessionStore::new(pool))
    }

    async fn bound_session_level(config_level: crate::permission::Level) -> String {
        let session_store = test_session_store().await;
        let action = super::super::ensure_action_session(
            CronAction::SendMessage {
                session_id: None,
                content: "hi".to_string(),
            },
            "nightly",
            &session_store,
            None,
            config_level,
        )
        .await
        .unwrap();
        let sid = super::super::action_session_id(&action).expect("session bound");
        session_store
            .get(&sid)
            .await
            .unwrap()
            .and_then(|i| i.auto_approve_level)
            .expect("auto_approve_level persisted")
    }

    #[tokio::test]
    async fn cron_session_level_follows_config_floored_at_caution() {
        use crate::permission::Level;
        // safe/caution 都被抬到 caution（无人值守，低了会卡在批准上）
        assert_eq!(bound_session_level(Level::Safe).await, "caution");
        assert_eq!(bound_session_level(Level::Caution).await, "caution");
        // 更高的配置原样保留
        assert_eq!(bound_session_level(Level::Dangerous).await, "dangerous");
    }
}
