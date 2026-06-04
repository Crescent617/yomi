#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::types::{
        CronAction, CronError, CronJob, CronJobId, CronJobStatus, CronSchedule,
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
        // Should be at 9:00 AM
        assert_eq!(next.hour(), 9);
        assert_eq!(next.minute(), 0);
        assert_eq!(next.second(), 0);
    }

    #[test]
    fn test_cron_schedule_upcoming() {
        let schedule = CronSchedule::parse("0 0 9 * * *").unwrap();
        let now = Utc::now();
        let upcoming = schedule.upcoming(now, 3);
        assert_eq!(upcoming.len(), 3);
        for t in &upcoming {
            assert_eq!(t.hour(), 9);
            assert_eq!(t.minute(), 0);
            assert_eq!(t.second(), 0);
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
            session_id: "test-session".to_string(),
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
            max_runs: Some(10),
            expires_at: None,
            last_error: None,
        };

        let json = serde_json::to_string(&job).unwrap();
        let decoded: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(job.name, decoded.name);
        assert_eq!(job.schedule, decoded.schedule);
        assert_eq!(job.status, decoded.status);
    }
}
