use kernel::types::{Project, ProjectId};
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_projects(state: State<'_, AppState>) -> Result<Vec<Project>, GuiError> {
    let coord = state.kernel.clone();
    let projects = coord.list_projects().await.map_err(GuiError::kernel)?;
    Ok(projects)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_project(
    state: State<'_, AppState>,
    dir: String,
    name: Option<String>,
) -> Result<Project, GuiError> {
    let coord = state.kernel.clone();
    let project = coord
        .create_project(dir.into(), name)
        .await
        .map_err(GuiError::kernel)?;
    Ok(project)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Option<Project>, GuiError> {
    let coord = state.kernel.clone();
    let project = coord
        .get_project(&ProjectId::from(project_id))
        .await
        .map_err(GuiError::kernel)?;
    Ok(project)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rename_project(
    state: State<'_, AppState>,
    project_id: String,
    name: String,
) -> Result<(), GuiError> {
    let coord = state.kernel.clone();
    coord
        .rename_project(&ProjectId::from(project_id), name)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

/// Summary of a cascade project deletion, for frontend display
#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DeleteProjectResult {
    pub sessions_deleted: usize,
    pub bytes_reclaimed: u64,
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<DeleteProjectResult, GuiError> {
    let coord = state.kernel.clone();
    let report = coord
        .delete_project(&ProjectId::from(project_id))
        .await
        .map_err(GuiError::kernel)?;
    Ok(DeleteProjectResult {
        sessions_deleted: report.sessions.len(),
        bytes_reclaimed: report.bytes_reclaimed,
    })
}
