use kernel::client::PaginatedSessions;
use kernel::permission::Level;
use kernel::types::SessionId;
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_sessions(
    state: State<'_, AppState>,
    project_id: Option<String>,
    scope: kernel::storage::session::SessionListScope,
    before: Option<String>,
    limit: Option<usize>,
) -> Result<PaginatedSessions, GuiError> {
    let coord = state.kernel_snapshot();

    let pid = project_id.map(kernel::types::ProjectId::from);
    let before_dt = match before {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(&s)
                .map_err(|e| GuiError::unknown(format!("Invalid before date: {e}")))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };
    let limit = limit.unwrap_or(50);

    let result = coord
        .list_sessions(pid.as_ref(), scope, before_dt, limit)
        .await
        .map_err(GuiError::kernel)?;

    Ok(result)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_running_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<kernel::types::RunningSessionResponse>, GuiError> {
    state
        .kernel_snapshot()
        .list_running_sessions()
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_subagents(
    state: State<'_, AppState>,
    parent_session_id: String,
) -> Result<Vec<kernel::types::SubagentResponse>, GuiError> {
    state
        .kernel_snapshot()
        .list_subagents(&SessionId::from(parent_session_id))
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_session(
    state: State<'_, AppState>,
    project_id: Option<String>,
    working_dir: Option<String>,
    auto_approve_level: String,
    model_key: Option<String>,
) -> Result<String, GuiError> {
    let coord = state.kernel_snapshot();
    let level = parse_level(&auto_approve_level)?;
    let input = kernel::CreateSessionInput {
        project_id: project_id.map(kernel::types::ProjectId::from),
        working_dir: working_dir.map(std::path::PathBuf::from),
        auto_approve_level: level,
        tool_blocklist: vec![],
        model_key,
    };
    let session_id = coord
        .create_session(input)
        .await
        .map_err(GuiError::kernel)?;
    Ok(session_id.0.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn restore_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = SessionId::from(session_id);
    coord
        .restore_session(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn fork_session(
    state: State<'_, AppState>,
    parent_id: String,
    auto_approve_level: String,
) -> Result<String, GuiError> {
    let coord = state.kernel_snapshot();
    let level = parse_level(&auto_approve_level)?;
    let pid = SessionId::from(parent_id);
    let new_id = coord
        .fork_session(&pid, level)
        .await
        .map_err(GuiError::kernel)?;
    Ok(new_id.0.to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = SessionId::from(session_id);
    coord.delete_session(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn shutdown_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = SessionId::from(session_id);
    let _ = coord.cancel(&sid).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pin_session(
    state: State<'_, AppState>,
    session_id: String,
    icon_emoji: Option<String>,
) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = SessionId::from(session_id);
    coord
        .pin_session(&sid, icon_emoji)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn unpin_session(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = SessionId::from(session_id);
    coord.unpin_session(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_pinned_session_emoji(
    state: State<'_, AppState>,
    session_id: String,
    icon_emoji: Option<String>,
) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = SessionId::from(session_id);
    coord
        .set_pinned_session_emoji(&sid, icon_emoji)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_pinned_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<kernel::storage::pinned_session::PinnedSessionDetail>, GuiError> {
    let coord = state.kernel_snapshot();
    let result = coord
        .list_pinned_sessions()
        .await
        .map_err(GuiError::kernel)?;
    Ok(result)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn clear_session(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    let coord = state.kernel_snapshot();
    let sid = SessionId::from(session_id);
    coord.clear_session(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

fn parse_level(s: &str) -> Result<Level, GuiError> {
    match s.to_lowercase().as_str() {
        "safe" => Ok(Level::Safe),
        "caution" => Ok(Level::Caution),
        "dangerous" => Ok(Level::Dangerous),
        _ => Err(GuiError::unknown(format!("Unknown permission level: {s}"))),
    }
}
