use kernel::permissions::Level;
use kernel::types::SessionId;
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub parent_id: Option<String>,
    pub title: Option<String>,
    pub message_count: i64,
    #[serde(rename = "projectId")]
    pub project_id: Option<String>,
    #[serde(rename = "workingDir")]
    pub working_dir: Option<String>,
}

#[derive(serde::Serialize)]
pub struct PaginatedSessions {
    pub sessions: Vec<SessionInfo>,
    pub has_more: bool,
}

#[tauri::command]
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

    Ok(PaginatedSessions {
        sessions: result
            .sessions
            .into_iter()
            .map(|s| SessionInfo {
                id: s.id.0,
                created_at: s.created_at.to_rfc3339(),
                updated_at: s.updated_at.to_rfc3339(),
                parent_id: s.parent_id.map(|p| p.0),
                title: s.title,
                message_count: s.message_count,
                project_id: s.project_id.map(|p| p.0),
                working_dir: s.working_dir,
            })
            .collect(),
        has_more: result.has_more,
    })
}

#[tauri::command]
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

#[tauri::command]
pub async fn restore_session(
    state: State<'_, AppState>,
    session_id: String,
    auto_approve_level: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let level = parse_level(&auto_approve_level)?;
    let sid = SessionId(session_id);
    coord
        .restore_session(&sid, level)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command]
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

#[tauri::command]
pub async fn delete_session(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .delete_session(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command]
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

fn parse_level(s: &str) -> Result<Level, GuiError> {
    match s.to_lowercase().as_str() {
        "safe" => Ok(Level::Safe),
        "caution" => Ok(Level::Caution),
        "dangerous" => Ok(Level::Dangerous),
        _ => Err(GuiError::unknown(format!("Unknown permission level: {s}"))),
    }
}
