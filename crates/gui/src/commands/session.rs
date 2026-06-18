use kernel::client::PaginatedSessions;
use kernel::permissions::Level;
use kernel::types::SessionId;
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_sessions(
    state: State<'_, AppState>,
    project_id: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
) -> Result<PaginatedSessions, GuiError> {
    let coord = state.coordinator.clone();

    let pid = project_id.map(kernel::types::ProjectId);
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
        .list_sessions(pid.as_ref(), before_dt, limit)
        .await
        .map_err(GuiError::kernel)?;

    Ok(result)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_session(
    state: State<'_, AppState>,
    project_id: Option<String>,
    working_dir: Option<String>,
    auto_approve_level: String,
) -> Result<String, GuiError> {
    let coord = state.coordinator.clone();
    let level = parse_level(&auto_approve_level)?;
    let input = kernel::CreateSessionInput {
        project_id: project_id.map(kernel::types::ProjectId),
        working_dir: working_dir.map(std::path::PathBuf::from),
        auto_approve_level: level,
    };
    let session_id = coord
        .create_session(input)
        .await
        .map_err(GuiError::kernel)?;
    Ok(session_id.0)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn restore_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
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
    let coord = state.coordinator.clone();
    let level = parse_level(&auto_approve_level)?;
    let pid = SessionId(parent_id);
    let new_id = coord
        .fork_session(&pid, level)
        .await
        .map_err(GuiError::kernel)?;
    Ok(new_id.0)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord.delete_session(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn shutdown_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .shutdown_session(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn pin_session(
    state: State<'_, AppState>,
    session_id: String,
    icon_emoji: Option<String>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .pin_session(&sid, icon_emoji)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn unpin_session(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord.unpin_session(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_pinned_session_emoji(
    state: State<'_, AppState>,
    session_id: String,
    icon_emoji: Option<String>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
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
    let coord = state.coordinator.clone();
    let result = coord
        .list_pinned_sessions()
        .await
        .map_err(GuiError::kernel)?;
    Ok(result)
}

fn parse_level(s: &str) -> Result<Level, GuiError> {
    match s.to_lowercase().as_str() {
        "safe" => Ok(Level::Safe),
        "caution" => Ok(Level::Caution),
        "dangerous" => Ok(Level::Dangerous),
        _ => Err(GuiError::unknown(format!("Unknown permission level: {s}"))),
    }
}
