use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command]
pub async fn list_skills(_state: State<'_, AppState>) -> Result<Vec<serde_json::Value>, GuiError> {
    // TODO: Kernel wire protocol does not expose list_skills yet.
    // Return empty until a remote API is added.
    Ok(vec![])
}

#[tauri::command]
pub async fn reload_config(state: State<'_, AppState>) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    coord
        .reload_agent_config()
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}
