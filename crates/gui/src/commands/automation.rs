use kernel::cron::{CronAction, CronJob, CronJobId, CronJobStatus, UpdateCronJobInput};
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

// ── GUI-layer camelCase wrappers for CronJob ───────────────────────────

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CronJobInfo {
    id: String,
    name: String,
    schedule: String,
    action: CronAction,
    status: String,
    created_at: String,
    updated_at: String,
    next_run_at: Option<String>,
    last_run_at: Option<String>,
    run_count: u32,
    max_runs: Option<u32>,
    expires_at: Option<String>,
    last_error: Option<String>,
}

fn cron_job_info(job: &CronJob) -> CronJobInfo {
    CronJobInfo {
        id: job.id.0.clone(),
        name: job.name.clone(),
        schedule: job.schedule.clone(),
        action: job.action.clone(),
        status: job.status.as_str().to_string(),
        created_at: job.created_at.to_rfc3339(),
        updated_at: job.updated_at.to_rfc3339(),
        next_run_at: job.next_run_at.map(|d| d.to_rfc3339()),
        last_run_at: job.last_run_at.map(|d| d.to_rfc3339()),
        run_count: job.run_count,
        max_runs: job.max_runs,
        expires_at: job.expires_at.map(|d| d.to_rfc3339()),
        last_error: job.last_error.clone(),
    }
}

#[tauri::command]
pub async fn list_cron_jobs(
    state: State<'_, AppState>,
    status: Option<String>,
    limit: usize,
) -> Result<Vec<serde_json::Value>, GuiError> {
    let store = state
        .cron_store
        .as_ref()
        .ok_or_else(|| GuiError::unknown("cron store not configured"))?;

    let status_parsed = match status {
        Some(s) => Some(s.parse::<CronJobStatus>().map_err(GuiError::unknown)?),
        None => None,
    };

    let jobs = store
        .list(status_parsed, limit)
        .await
        .map_err(|e| GuiError::kernel(format!("list cron jobs failed: {e}")))?;

    let values: Vec<serde_json::Value> = jobs
        .into_iter()
        .map(|job| {
            let info = cron_job_info(&job);
            serde_json::to_value(info).map_err(GuiError::unknown)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(values)
}

#[tauri::command]
pub async fn create_cron_job(
    state: State<'_, AppState>,
    name: String,
    schedule: String,
    action: serde_json::Value,
    max_runs: Option<u32>,
    expires_at: Option<String>,
) -> Result<String, GuiError> {
    let store = state
        .cron_store
        .as_ref()
        .ok_or_else(|| GuiError::unknown("cron store not configured"))?;

    // Validate schedule syntax
    let _ = kernel::cron::CronSchedule::parse(&schedule)
        .map_err(|e| GuiError::unknown(format!("invalid schedule: {e}")))?;

    let action: CronAction = serde_json::from_value(action)
        .map_err(|e| GuiError::unknown(format!("invalid action: {e}")))?;

    let expires_at = match expires_at {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(&s)
                .map_err(|e| GuiError::unknown(format!("invalid expires_at: {e}")))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };

    let now = chrono::Utc::now();
    let next_run = kernel::cron::CronSchedule::parse(&schedule)
        .ok()
        .and_then(|s| s.next_after(now));

    let job = CronJob {
        id: CronJobId::new(),
        name,
        schedule,
        action,
        status: CronJobStatus::Active,
        created_at: now,
        updated_at: now,
        next_run_at: next_run,
        last_run_at: None,
        run_count: 0,
        max_runs,
        expires_at,
        last_error: None,
    };

    let job_id = job.id.clone();
    store
        .create(&job)
        .await
        .map_err(|e| GuiError::kernel(format!("create cron job failed: {e}")))?;

    if let Some(ref scheduler) = state.cron_scheduler {
        scheduler.reload();
    }

    Ok(job_id.0)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_cron_job(
    state: State<'_, AppState>,
    job_id: String,
    name: Option<String>,
    schedule: Option<String>,
    action: Option<serde_json::Value>,
    status: Option<String>,
    max_runs: Option<u32>,
    expires_at: Option<String>,
) -> Result<(), GuiError> {
    let store = state
        .cron_store
        .as_ref()
        .ok_or_else(|| GuiError::unknown("cron store not configured"))?;

    // Validate schedule if provided
    if let Some(ref s) = schedule {
        let _ = kernel::cron::CronSchedule::parse(s)
            .map_err(|e| GuiError::unknown(format!("invalid schedule: {e}")))?;
    }

    let status_parsed = match status {
        Some(s) => Some(s.parse::<CronJobStatus>().map_err(GuiError::unknown)?),
        None => None,
    };

    let action_parsed = match action {
        Some(v) => Some(serde_json::from_value::<CronAction>(v)
            .map_err(|e| GuiError::unknown(format!("invalid action: {e}")))?),
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
        action: action_parsed,
        status: status_parsed,
        max_runs,
        expires_at: expires_at_parsed,
        ..Default::default()
    };

    store
        .update(&CronJobId(job_id), &input)
        .await
        .map_err(|e| GuiError::kernel(format!("update cron job failed: {e}")))?;

    if let Some(ref scheduler) = state.cron_scheduler {
        scheduler.reload();
    }

    Ok(())
}

#[tauri::command]
pub async fn delete_cron_job(state: State<'_, AppState>, job_id: String) -> Result<(), GuiError> {
    let store = state
        .cron_store
        .as_ref()
        .ok_or_else(|| GuiError::unknown("cron store not configured"))?;

    store
        .delete(&CronJobId(job_id))
        .await
        .map_err(|e| GuiError::kernel(format!("delete cron job failed: {e}")))?;

    if let Some(ref scheduler) = state.cron_scheduler {
        scheduler.reload();
    }

    Ok(())
}

#[tauri::command]
pub async fn trigger_cron_job(state: State<'_, AppState>, job_id: String) -> Result<(), GuiError> {
    let store = state
        .cron_store
        .as_ref()
        .ok_or_else(|| GuiError::unknown("cron store not configured"))?;

    let job = store
        .get(&CronJobId(job_id.clone()))
        .await
        .map_err(|e| GuiError::kernel(format!("get cron job failed: {e}")))?
        .ok_or_else(|| GuiError::unknown("cron job not found"))?;

    // Execute the action directly (mirrors CronWorker::execute)
    let result = execute_cron_action(state.coordinator.as_ref(), &job.action).await;

    let error = match &result {
        Ok(()) => None,
        Err(e) => {
            tracing::error!("Cron job {} trigger failed: {}", job_id, e);
            Some(e.clone())
        }
    };

    store
        .record_execution(&CronJobId(job_id), error)
        .await
        .map_err(|e| GuiError::kernel(format!("record execution failed: {e}")))?;

    result.map_err(GuiError::unknown)
}

async fn execute_cron_action(
    coordinator: &dyn kernel::client::CoordinatorApi,
    action: &CronAction,
) -> Result<(), String> {
    match action {
        CronAction::SendMessage {
            session_id,
            content,
        } => {
            let sid = kernel::types::SessionId(session_id.clone());
            let text = render_template(content);
            let blocks = vec![kernel::types::ContentBlock::Text { text }];
            coordinator
                .send_message(&sid, blocks)
                .await
                .map_err(|e| format!("send message failed: {e}"))?;
        }
        CronAction::Shell {
            command,
            working_dir,
        } => {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(working_dir.as_deref().unwrap_or("."))
                .kill_on_drop(true)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await
                .map_err(|e| format!("shell command failed: {e}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("shell command failed: {stderr}"));
            }
        }
        CronAction::Internal { .. } => {
            return Err("Internal cron actions cannot be triggered manually".to_string());
        }
    }
    Ok(())
}

fn render_template(template: &str) -> String {
    let now = chrono::Utc::now();
    template
        .replace("{{timestamp}}", &now.to_rfc3339())
        .replace("{{date}}", &now.format("%Y-%m-%d").to_string())
        .replace("{{time}}", &now.format("%H:%M:%S").to_string())
}
