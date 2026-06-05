use tauri::{AppHandle, State};

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command]
pub async fn terminal_spawn(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    id: String,
    cwd: String,
    cols: u16,
    rows: u16,
) -> Result<(), GuiError> {
    let manager = state.terminal_manager.lock().await;
    manager
        .spawn(id, std::path::Path::new(&cwd), app_handle, cols, rows)
        .await
}

#[tauri::command]
pub async fn terminal_write(
    state: State<'_, AppState>,
    id: String,
    data: String,
) -> Result<(), GuiError> {
    let manager = state.terminal_manager.lock().await;
    manager.write(&id, &data).await
}

#[tauri::command]
pub async fn terminal_resize(
    state: State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), GuiError> {
    let manager = state.terminal_manager.lock().await;
    manager.resize(&id, cols, rows).await
}

#[tauri::command]
pub async fn terminal_kill(state: State<'_, AppState>, id: String) -> Result<(), GuiError> {
    let manager = state.terminal_manager.lock().await;
    manager.kill(&id).await
}
