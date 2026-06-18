use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_session_skills(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<serde_json::Value>, GuiError> {
    let coord = state.coordinator.clone();
    let skills = coord
        .list_session_skills(&kernel::types::SessionId(session_id))
        .await
        .map_err(GuiError::kernel)?;
    let values = skills
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
            })
        })
        .collect();
    Ok(values)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reload_config(state: State<'_, AppState>) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    coord
        .reload_agent_config()
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}
