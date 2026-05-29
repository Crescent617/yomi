use kernel::checkpoint::RewindTarget;
use kernel::client::CoordinatorApi;
use kernel::types::MessageId;
use kernel::types::SessionId;
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command]
pub async fn get_checkpoints(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<serde_json::Value>, GuiError> {
    let coord = state.get_or_connect().await?;
    let sid = SessionId(session_id);
    let checkpoints = coord
        .get_checkpoints(&sid)
        .await
        .map_err(GuiError::kernel)?;
    let values: Vec<_> = checkpoints
        .into_iter()
        .map(|c| serde_json::to_value(c).unwrap_or_default())
        .collect();
    Ok(values)
}

#[tauri::command]
pub async fn rewind(
    state: State<'_, AppState>,
    session_id: String,
    message_id: String,
) -> Result<(), GuiError> {
    let coord = state.get_or_connect().await?;
    let sid = SessionId(session_id);
    let mid = MessageId::from_string(message_id);
    coord
        .rewind_session(&sid, mid, RewindTarget::Both)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}
