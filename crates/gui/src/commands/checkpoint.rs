use kernel::checkpoint::{Checkpoint, RewindTarget};
use kernel::types::MessageId;
use kernel::types::SessionId;
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn get_checkpoints(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<Checkpoint>, GuiError> {
    let coord = state.kernel.clone();
    let sid = SessionId::from(session_id);
    let checkpoints = coord
        .get_checkpoints(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(checkpoints)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rewind(
    state: State<'_, AppState>,
    session_id: String,
    message_id: String,
) -> Result<(), GuiError> {
    let coord = state.kernel.clone();
    let sid = SessionId::from(session_id);
    let mid = MessageId::from(message_id);
    coord
        .rewind_session(&sid, mid, RewindTarget::Both)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}
