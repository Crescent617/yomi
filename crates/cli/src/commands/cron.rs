//! `yomi cron` — manage daemon cron jobs via the wire API.
//!
//! All operations go through `KernelApi` (remote daemon), never the store
//! directly — see the design note on `KernelApi`'s cron section.

use crate::args::GlobalArgs;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use comfy_table::{ContentArrangement, Table};
use kernel::client::{KernelApi, RemoteKernel};
use kernel::cron::{
    CreateCronJobInput, CronAction, CronJob, CronJobId, CronJobStatus, CronSessionTemplate,
    UpdateCronJobInput,
};

async fn connect() -> Result<RemoteKernel> {
    crate::daemon::connect_strict().await
}

/// Parse an expiry timestamp: RFC 3339 (e.g. `2026-08-01T09:00:00+08:00`),
/// or "never" / the zero timestamp for the no-expiry sentinel.
fn parse_expires_at(raw: &str) -> Result<DateTime<Utc>> {
    if raw.eq_ignore_ascii_case("never") {
        return Ok(kernel::cron::NEVER_EXPIRES);
    }
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| {
            format!("Invalid --expires-at timestamp: {raw} (expected RFC 3339 or 'never')")
        })
}

/// Build the action from `--message` / `--command` flags (exactly one
/// required on create; both absent on update keeps the current action).
///
/// `--work-dir` 对 message 类（未绑 `--session`）经 `session_template`
/// 透传为 per-run 新会话的工作目录——kernel 尊重调用方的目录、只重算
/// 权限等级（见 `create_cron_job`/`update_cron_job` 的归一化）。绑定
/// `--session` 时 `--work-dir` 无意义：显式报错而非静默忽略。
fn build_action(
    message: Option<String>,
    command: Option<String>,
    session: Option<String>,
    work_dir: Option<String>,
) -> Result<Option<CronAction>> {
    Ok(match (message, command) {
        (Some(content), None) => {
            if session.is_some() && work_dir.is_some() {
                anyhow::bail!(
                    "--work-dir has no effect with --message + --session \
                     (the bound session keeps its own working directory)"
                );
            }
            Some(CronAction::SendMessage {
                session_id: session,
                content,
                session_template: work_dir.map(|dir| CronSessionTemplate {
                    working_dir: Some(dir),
                    project_id: None,
                    auto_approve_level: None,
                }),
            })
        }
        (None, Some(command)) => Some(CronAction::Shell {
            command,
            working_dir: work_dir,
        }),
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!("clap conflicts_with"),
    })
}

/// One-line truncation for table cells.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::build_action;
    use kernel::cron::CronAction;

    #[test]
    fn message_with_work_dir_fills_session_template() {
        let action = build_action(
            Some("hi".to_string()),
            None,
            None,
            Some("/proj".to_string()),
        )
        .unwrap()
        .unwrap();
        let CronAction::SendMessage {
            session_template, ..
        } = action
        else {
            panic!("expected SendMessage");
        };
        let tpl = session_template.expect("template filled from --work-dir");
        assert_eq!(tpl.working_dir.as_deref(), Some("/proj"));
        // 权限等级留给 kernel 按 config 重算（下限 caution），CLI 不预设。
        assert!(tpl.auto_approve_level.is_none());
        assert!(tpl.project_id.is_none());
    }

    #[test]
    fn message_without_work_dir_keeps_template_absent() {
        let action = build_action(Some("hi".to_string()), None, None, None)
            .unwrap()
            .unwrap();
        let CronAction::SendMessage {
            session_template, ..
        } = action
        else {
            panic!("expected SendMessage");
        };
        assert!(session_template.is_none());
    }

    #[test]
    fn message_with_session_and_work_dir_is_rejected() {
        // 绑定会话有自己的工作目录，--work-dir 无意义——显式报错而非静默忽略。
        let err = build_action(
            Some("hi".to_string()),
            None,
            Some("sess_1".to_string()),
            Some("/proj".to_string()),
        )
        .unwrap_err();
        assert!(err.to_string().contains("--work-dir"), "{err}");
    }

    #[test]
    fn shell_work_dir_unchanged() {
        let action = build_action(None, Some("ls".to_string()), None, Some("/x".to_string()))
            .unwrap()
            .unwrap();
        let CronAction::Shell { working_dir, .. } = action else {
            panic!("expected Shell");
        };
        assert_eq!(working_dir.as_deref(), Some("/x"));
    }
}

/// One-line action summary for the list table.
fn action_summary(action: &CronAction) -> String {
    match action {
        CronAction::SendMessage {
            session_id,
            content,
            ..
        } => {
            let target = session_id.as_deref().unwrap_or("fresh session per run");
            format!("msg → {} · {}", target, truncate(content, 40))
        }
        CronAction::Shell {
            command,
            working_dir,
        } => {
            let dir = working_dir
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default();
            format!("sh{} · {}", dir, truncate(command, 40))
        }
        CronAction::Internal { endpoint, .. } => format!("internal · {endpoint}"),
    }
}

fn fmt_local(dt: &DateTime<Utc>) -> String {
    dt.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[allow(clippy::too_many_arguments)]
pub async fn create(
    _global: &GlobalArgs,
    name: String,
    schedule: String,
    message: Option<String>,
    command: Option<String>,
    session: Option<String>,
    work_dir: Option<String>,
    max_runs: Option<u32>,
    expires_at: Option<String>,
    precheck: Option<String>,
) -> Result<()> {
    let action = build_action(message, command, session, work_dir)?
        .expect("create requires --message or --command");
    let expires_at = expires_at.as_deref().map(parse_expires_at).transpose()?;

    let kernel = connect().await?;
    let id = kernel
        .create_cron_job(CreateCronJobInput {
            name,
            schedule,
            action,
            max_runs,
            expires_at,
            precheck,
        })
        .await
        .context("Failed to create cron job")?;
    println!("Created cron job {id}");
    Ok(())
}

pub async fn list(_global: &GlobalArgs, status: Option<String>, limit: usize) -> Result<()> {
    let status = status
        .as_deref()
        .map(|s| s.parse::<CronJobStatus>().map_err(|e| anyhow::anyhow!(e)))
        .transpose()?;

    let kernel = connect().await?;
    let jobs = kernel
        .list_cron_jobs(status, limit)
        .await
        .context("Failed to list cron jobs")?;

    if jobs.is_empty() {
        println!("No cron jobs found.");
        return Ok(());
    }

    let mut table = Table::new();
    table
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            "ID", "NAME", "SCHEDULE", "STATUS", "NEXT RUN", "RUNS", "ACTION",
        ]);
    table.load_preset(comfy_table::presets::NOTHING);
    if let Some(col) = table.column_mut(0) {
        col.set_padding((0, 1));
    }

    for job in &jobs {
        let next_run = job
            .next_run_at
            .as_ref()
            .map_or_else(|| "-".to_string(), fmt_local);
        let runs = if job.has_max_runs() {
            format!("{}/{}", job.run_count, job.max_runs)
        } else {
            job.run_count.to_string()
        };
        table.add_row(vec![
            job.id.to_string(),
            job.name.clone(),
            job.schedule.clone(),
            job.status.as_str().to_string(),
            next_run,
            runs,
            {
                let mut summary = action_summary(&job.action);
                if let Some(ref pre) = job.precheck {
                    summary.push_str(" [when: ");
                    summary.push_str(&truncate(pre, 24));
                    summary.push(']');
                }
                summary
            },
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn get(_global: &GlobalArgs, job_id: String) -> Result<()> {
    let kernel = connect().await?;
    let job: Option<CronJob> = kernel
        .get_cron_job(&CronJobId::from(job_id.clone()))
        .await
        .context("Failed to get cron job")?;
    match job {
        Some(job) => println!("{}", serde_json::to_string_pretty(&job)?),
        None => anyhow::bail!("Cron job not found: {job_id}"),
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn update(
    _global: &GlobalArgs,
    job_id: String,
    name: Option<String>,
    schedule: Option<String>,
    message: Option<String>,
    command: Option<String>,
    session: Option<String>,
    work_dir: Option<String>,
    max_runs: Option<u32>,
    expires_at: Option<String>,
    precheck: Option<String>,
) -> Result<()> {
    let action = build_action(message, command, session, work_dir)?;
    let expires_at = expires_at.as_deref().map(parse_expires_at).transpose()?;

    let kernel = connect().await?;
    let updated = kernel
        .update_cron_job(
            &CronJobId::from(job_id.clone()),
            UpdateCronJobInput {
                name,
                schedule,
                action,
                status: None,
                max_runs,
                expires_at,
                precheck,
                next_run_at: None,
            },
        )
        .await
        .context("Failed to update cron job")?;
    if updated {
        println!("Cron job {job_id} updated.");
    } else {
        anyhow::bail!("Cron job not found: {job_id}");
    }
    Ok(())
}

pub async fn set_status(_global: &GlobalArgs, job_id: String, status: CronJobStatus) -> Result<()> {
    let kernel = connect().await?;
    let updated = kernel
        .update_cron_job(
            &CronJobId::from(job_id.clone()),
            UpdateCronJobInput {
                status: Some(status),
                ..Default::default()
            },
        )
        .await
        .context("Failed to update cron job status")?;
    if updated {
        println!("Cron job {job_id} is now {}.", status.as_str());
    } else {
        anyhow::bail!("Cron job not found: {job_id}");
    }
    Ok(())
}

pub async fn delete(_global: &GlobalArgs, job_id: String) -> Result<()> {
    let kernel = connect().await?;
    let deleted = kernel
        .delete_cron_job(&CronJobId::from(job_id.clone()))
        .await
        .context("Failed to delete cron job")?;
    if deleted {
        println!("Cron job {job_id} deleted.");
    } else {
        anyhow::bail!("Cron job not found: {job_id}");
    }
    Ok(())
}

pub async fn trigger(_global: &GlobalArgs, job_id: String) -> Result<()> {
    let kernel = connect().await?;
    kernel
        .trigger_cron_job(&CronJobId::from(job_id.clone()))
        .await
        .context("Failed to trigger cron job")?;
    println!("Cron job {job_id} triggered.");
    Ok(())
}
