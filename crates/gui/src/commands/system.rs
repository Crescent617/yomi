use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command]
pub async fn ping(state: State<'_, AppState>) -> Result<bool, GuiError> {
    match state.get_or_connect().await {
        Ok(_) => Ok(true),
        Err(_) => {
            state.disconnect().await;
            Ok(false)
        }
    }
}

#[tauri::command]
pub fn get_cwd() -> Result<String, GuiError> {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| GuiError::unknown(format!("Failed to get cwd: {e}")))
}
