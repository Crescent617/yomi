use kernel::cron::{CronAction, CronJob, CronJobId, CronJobStatus, UpdateCronJobInput};
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_cron_jobs(
    state: State<'_, AppState>,
    status: Option<String>,
    limit: usize,
) -> Result<Vec<CronJob>, GuiError> {
    let status_parsed = match status {
        Some(s) => Some(s.parse::<CronJobStatus>().map_err(GuiError::unknown)?),
        None => None,
    };

    let jobs = state
        .kernel_snapshot()
        .list_cron_jobs(status_parsed, limit)
        .await
        .map_err(|e| GuiError::kernel(format!("list cron jobs failed: {e}")))?;

    Ok(jobs)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_cron_job(
    state: State<'_, AppState>,
    name: String,
    schedule: String,
    action: String,
    max_runs: Option<u32>,
    expires_at: Option<String>,
) -> Result<String, GuiError> {
    kernel::cron::CronSchedule::parse(&schedule)
        .map_err(|e| GuiError::unknown(format!("invalid schedule: {e}")))?;

    let action: CronAction = serde_json::from_str(&action)
        .map_err(|e| GuiError::unknown(format!("invalid action: {e}")))?;

    let expires_at = match expires_at {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(&s)
                .map_err(|e| GuiError::unknown(format!("invalid expires_at: {e}")))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };

    let input = kernel::cron::CreateCronJobInput {
        name,
        schedule,
        action,
        max_runs,
        expires_at,
    };

    let job_id = state
        .kernel_snapshot()
        .create_cron_job(input)
        .await
        .map_err(|e| GuiError::kernel(format!("create cron job failed: {e}")))?;

    Ok(job_id.0.to_string())
}

#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
pub async fn update_cron_job(
    state: State<'_, AppState>,
    job_id: String,
    name: Option<String>,
    schedule: Option<String>,
    action: Option<String>,
    status: Option<String>,
    max_runs: Option<u32>,
    expires_at: Option<String>,
    clear_max_runs: Option<bool>,
    clear_expires_at: Option<bool>,
) -> Result<(), GuiError> {
    if let Some(ref s) = schedule {
        kernel::cron::CronSchedule::parse(s)
            .map_err(|e| GuiError::unknown(format!("invalid schedule: {e}")))?;
    }

    let status_parsed = match status {
        Some(s) => Some(s.parse::<CronJobStatus>().map_err(GuiError::unknown)?),
        None => None,
    };

    let action = match action {
        Some(s) => Some(
            serde_json::from_str::<CronAction>(&s)
                .map_err(|e| GuiError::unknown(format!("invalid action: {e}")))?,
        ),
        None => None,
    };

    let expires_at_parsed = match expires_at {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(&s)
                .map_err(|e| GuiError::unknown(format!("invalid expires_at: {e}")))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };

    let input = UpdateCronJobInput {
        name,
        schedule,
        action,
        status: status_parsed,
        max_runs,
        expires_at: expires_at_parsed,
        clear_max_runs: clear_max_runs.unwrap_or(false),
        clear_expires_at: clear_expires_at.unwrap_or(false),
        ..Default::default()
    };

    state
        .kernel_snapshot()
        .update_cron_job(&CronJobId::from(job_id), input)
        .await
        .map_err(|e| GuiError::kernel(format!("update cron job failed: {e}")))?;

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_cron_job(state: State<'_, AppState>, job_id: String) -> Result<(), GuiError> {
    state
        .kernel_snapshot()
        .delete_cron_job(&CronJobId::from(job_id))
        .await
        .map_err(|e| GuiError::kernel(format!("delete cron job failed: {e}")))?;

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn trigger_cron_job(state: State<'_, AppState>, job_id: String) -> Result<(), GuiError> {
    state
        .kernel_snapshot()
        .trigger_cron_job(&CronJobId::from(job_id))
        .await
        .map_err(|e| GuiError::kernel(format!("trigger cron job failed: {e}")))?;

    Ok(())
}
