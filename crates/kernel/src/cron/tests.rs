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
            session_template: None,
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("send_message"));
        assert!(json.contains("test-session"));

        let decoded: CronAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, decoded);

        // 旧格式（无 session_template 字段）向后兼容：反序列化为 None
        let legacy = r#"{"type":"send_message","session_id":"s","content":"c"}"#;
        let decoded: CronAction = serde_json::from_str(legacy).unwrap();
        assert!(matches!(
            decoded,
            CronAction::SendMessage {
                session_template: None,
                ..
            }
        ));
        // per-run 形态：session_id null + 模板，能完整往返
        let per_run = CronAction::SendMessage {
            session_id: None,
            content: "c".to_string(),
            session_template: Some(super::super::CronSessionTemplate {
                working_dir: Some("/w".into()),
                project_id: None,
                auto_approve_level: Some("caution".into()),
            }),
        };
        let json = serde_json::to_string(&per_run).unwrap();
        let decoded: CronAction = serde_json::from_str(&json).unwrap();
        assert_eq!(per_run, decoded);
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
            precheck: None,
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
            precheck: None,
        }
    }

    #[tokio::test]
    async fn create_cron_job_returns_existing_on_name_conflict() {
        let store = test_cron_store().await;

        let first = super::super::create_cron_job(
            &store,
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
            shell_input("janitor", "0 9 * * *"),
            crate::permission::Level::Safe,
        )
        .await
        .unwrap();

        assert!(!out.created);
        assert_eq!(out.job.id.0, "cron-winner");
        assert_eq!(store.list(None, 10).await.unwrap().len(), 1);
    }

    // ── per-run session 模板捕获与现场建会话 ─────────────────────────

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

    fn captured_level(config_level: crate::permission::Level) -> String {
        super::super::capture_session_template(None, config_level)
            .auto_approve_level
            .expect("level captured")
    }

    #[tokio::test]
    async fn session_template_level_follows_config_floored_at_caution() {
        use crate::permission::Level;
        // safe/caution 都被抬到 caution（无人值守，低了会卡在批准上）
        assert_eq!(captured_level(Level::Safe), "caution");
        assert_eq!(captured_level(Level::Caution), "caution");
        // 更高的配置原样保留
        assert_eq!(captured_level(Level::Dangerous), "dangerous");
    }

    #[tokio::test]
    async fn create_send_message_without_session_captures_template_only() {
        let store = test_cron_store().await;
        let follow = crate::storage::SessionInfo {
            id: crate::types::SessionId::from("sess-caller"),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            parent_id: None,
            title: None,
            message_count: 0,
            working_dir: Some("/repo/demo".into()),
            project_id: Some(crate::types::ProjectId::from("proj_1")),
            auto_approve_level: None,
            model_key: Some("custom-model".into()),
            template: None,
            settings: None,
        };

        let out = super::super::create_cron_job(
            &store,
            Some(&follow),
            super::super::CreateCronJobInput {
                name: "nightly".to_string(),
                schedule: "0 9 * * *".to_string(),
                action: CronAction::SendMessage {
                    session_id: None,
                    content: "hi".to_string(),
                    session_template: None,
                },
                max_runs: None,
                expires_at: None,
                precheck: None,
            },
            crate::permission::Level::Safe,
        )
        .await
        .unwrap();

        // 创建时不建会话、不回填 session_id，只捕获模板：
        // working_dir/project 跟随调用方，model 不继承，权限下限 caution。
        let CronAction::SendMessage {
            session_id,
            session_template: Some(tpl),
            ..
        } = out.job.action
        else {
            panic!("expected per-run send_message with template");
        };
        assert_eq!(session_id, None);
        assert_eq!(tpl.working_dir.as_deref(), Some("/repo/demo"));
        assert_eq!(
            tpl.project_id.as_ref().map(|p| p.0.to_string()).as_deref(),
            Some("proj_1")
        );
        assert_eq!(tpl.auto_approve_level.as_deref(), Some("caution"));
    }

    #[tokio::test]
    async fn spawn_run_session_creates_titled_session_from_template() {
        let session_store = test_session_store().await;
        let tpl = super::super::CronSessionTemplate {
            working_dir: Some("/repo/demo".into()),
            project_id: Some(crate::types::ProjectId::from("proj_1")),
            auto_approve_level: Some("caution".into()),
        };

        let sid = super::super::spawn_run_session(&session_store, Some(&tpl), "nightly")
            .await
            .unwrap();
        let info = session_store.get(&sid).await.unwrap().unwrap();
        assert!(info.title.as_deref().unwrap().starts_with("nightly · "));
        assert_eq!(info.working_dir.as_deref(), Some("/repo/demo"));
        assert_eq!(info.auto_approve_level.as_deref(), Some("caution"));
        // model 不设——跟随默认模型
        assert_eq!(info.model_key, None);

        // 两次运行 → 两个不同 session
        let sid2 = super::super::spawn_run_session(&session_store, Some(&tpl), "nightly")
            .await
            .unwrap();
        assert_ne!(sid.0, sid2.0);

        // 模板缺省时兜底 caution，不继承 cwd/project
        let sid3 = super::super::spawn_run_session(&session_store, None, "legacy")
            .await
            .unwrap();
        let info3 = session_store.get(&sid3).await.unwrap().unwrap();
        assert_eq!(info3.auto_approve_level.as_deref(), Some("caution"));
        assert_eq!(info3.working_dir, None);
    }

    #[tokio::test]
    async fn create_clamps_caller_supplied_template_level() {
        use crate::permission::Level;

        // RPC/GUI 调用方可以自带模板：cwd/project 尊重其值，但权限等级
        // 一律按 config 重算（下限 caution）——调用方给低给高都不信任
        let store = test_cron_store().await;
        for (i, (given, config, want)) in [
            ("safe", Level::Safe, "caution"),
            ("dangerous", Level::Safe, "caution"),
            ("safe", Level::Dangerous, "dangerous"),
        ]
        .into_iter()
        .enumerate()
        {
            let out = super::super::create_cron_job(
                &store,
                None,
                super::super::CreateCronJobInput {
                    name: format!("injected-{i}-{given}"),
                    schedule: "0 9 * * *".to_string(),
                    action: CronAction::SendMessage {
                        session_id: None,
                        content: "hi".to_string(),
                        session_template: Some(super::super::CronSessionTemplate {
                            working_dir: Some("/caller".into()),
                            project_id: None,
                            auto_approve_level: Some(given.to_string()),
                        }),
                    },
                    max_runs: None,
                    expires_at: None,
                    precheck: None,
                },
                config,
            )
            .await
            .unwrap();
            let CronAction::SendMessage {
                session_template: Some(tpl),
                ..
            } = out.job.action
            else {
                panic!("expected template");
            };
            assert_eq!(tpl.auto_approve_level.as_deref(), Some(want));
            assert_eq!(tpl.working_dir.as_deref(), Some("/caller"));
        }
    }

    // ── precheck 闸门 ────────────────────────────────────────────────

    #[tokio::test]
    async fn precheck_exit_zero_fires_with_stdout() {
        let dir = std::env::temp_dir();
        let out = super::super::run_precheck("echo new-arrivals", None, &dir).await;
        assert_eq!(
            out,
            super::super::PrecheckOutcome::Fire("new-arrivals".to_string())
        );
    }

    #[tokio::test]
    async fn precheck_nonzero_exit_skips() {
        let dir = std::env::temp_dir();
        assert_eq!(
            super::super::run_precheck("exit 1", None, &dir).await,
            super::super::PrecheckOutcome::Skip
        );
    }

    #[tokio::test]
    async fn precheck_missing_command_skips_fail_closed() {
        let dir = std::env::temp_dir();
        assert_eq!(
            super::super::run_precheck("definitely-not-a-command-xyz", None, &dir).await,
            super::super::PrecheckOutcome::Skip
        );
    }

    #[test]
    fn append_sensor_output_wraps_and_truncates() {
        let out = super::super::append_sensor_output("body", "reading-1\nreading-2");
        assert!(out.starts_with("body"));
        assert!(out.contains("Precheck output"));
        assert!(out.contains("reading-1\nreading-2"));

        let big = "x".repeat(super::super::MAX_SENSOR_STDOUT * 2);
        let out = super::super::append_sensor_output("body", &big);
        assert!(out.contains("[truncated]"));
        assert!(out.len() < big.len());
    }

    #[test]
    fn append_sensor_output_escapes_template_literals() {
        // 读数含模板字面量时必须原样保留（转义为空格变体，render_template
        // 只认无空格形式），否则模型看到的是被替换后的假数据。
        let out = super::super::append_sensor_output("body", "cron runs at {{date}} {{time}}");
        assert!(out.contains("{{ date }}"), "got: {out}");
        assert!(out.contains("{{ time }}"), "got: {out}");
        assert!(!out.contains("{{date}}"));
        // 非字面量不动
        let out = super::super::append_sensor_output("body", "{{other}} {{ date}}");
        assert!(out.contains("{{other}}"));
        assert!(out.contains("{{ date}}"));
    }

    #[tokio::test]
    async fn precheck_timeout_skips_fail_closed() {
        let dir = std::env::temp_dir();
        let out = super::super::run_precheck_with_timeout(
            "sleep 5",
            None,
            &dir,
            std::time::Duration::from_millis(200),
        )
        .await;
        assert_eq!(out, super::super::PrecheckOutcome::Skip);
    }
}
