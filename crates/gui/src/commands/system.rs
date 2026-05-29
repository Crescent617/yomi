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
