use kernel::types::{Project, ProjectId};
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_projects(state: State<'_, AppState>) -> Result<Vec<Project>, GuiError> {
    let coord = state.coordinator.clone();
    let projects = coord.list_projects().await.map_err(GuiError::kernel)?;
    Ok(projects)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_project(
    state: State<'_, AppState>,
    dir: String,
    name: Option<String>,
) -> Result<Project, GuiError> {
    let coord = state.coordinator.clone();
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
    let coord = state.coordinator.clone();
    let project = coord
        .get_project(&ProjectId(project_id))
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
    let coord = state.coordinator.clone();
    coord
        .rename_project(&ProjectId(project_id), name)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    coord
        .delete_project(&ProjectId(project_id))
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}
