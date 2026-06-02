use kernel::types::ProjectId;
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub created_at: String,
    pub updated_at: String,
}

#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectInfo>, GuiError> {
    let coord = state.coordinator.clone();
    let projects = coord
        .list_projects()
        .await
        .map_err(GuiError::kernel)?;
    Ok(projects
        .into_iter()
        .map(|p| ProjectInfo {
            id: p.id.0,
            name: p.name,
            dir: p.dir.to_string_lossy().to_string(),
            created_at: p.created_at.to_rfc3339(),
            updated_at: p.updated_at.to_rfc3339(),
        })
        .collect())
}

#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    dir: String,
    name: Option<String>,
) -> Result<ProjectInfo, GuiError> {
    let coord = state.coordinator.clone();
    let project = coord
        .create_project(dir.into(), name)
        .await
        .map_err(GuiError::kernel)?;
    Ok(ProjectInfo {
        id: project.id.0,
        name: project.name,
        dir: project.dir.to_string_lossy().to_string(),
        created_at: project.created_at.to_rfc3339(),
        updated_at: project.updated_at.to_rfc3339(),
    })
}

#[tauri::command]
pub async fn get_project(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Option<ProjectInfo>, GuiError> {
    let coord = state.coordinator.clone();
    let project = coord
        .get_project(&ProjectId(project_id))
        .await
        .map_err(GuiError::kernel)?;
    Ok(project.map(|p| ProjectInfo {
        id: p.id.0,
        name: p.name,
        dir: p.dir.to_string_lossy().to_string(),
        created_at: p.created_at.to_rfc3339(),
        updated_at: p.updated_at.to_rfc3339(),
    }))
}

#[tauri::command]
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

#[tauri::command]
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
