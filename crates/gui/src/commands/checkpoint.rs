use kernel::checkpoint::RewindTarget;
use kernel::types::MessageId;
use kernel::types::SessionId;
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointInfo {
    id: String,
    session_id: String,
    message_id: String,
    sequence: u32,
    created_at: u64,
    files_changed: usize,
    summary: String,
}

impl From<kernel::checkpoint::Checkpoint> for CheckpointInfo {
    fn from(cp: kernel::checkpoint::Checkpoint) -> Self {
        Self {
            id: cp.id,
            session_id: cp.session_id,
            message_id: cp.message_id,
            sequence: cp.sequence,
            created_at: cp.created_at,
            files_changed: cp.files_changed,
            summary: cp.summary,
        }
    }
}

#[tauri::command]
pub async fn get_checkpoints(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<serde_json::Value>, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let checkpoints = coord
        .get_checkpoints(&sid)
        .await
        .map_err(GuiError::kernel)?;
    let values: Vec<_> = checkpoints
        .into_iter()
        .map(|c| serde_json::to_value(CheckpointInfo::from(c)).unwrap_or_default())
        .collect();
    Ok(values)
}

#[tauri::command]
pub async fn rewind(
    state: State<'_, AppState>,
    session_id: String,
    message_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let mid = MessageId::from_string(message_id);
    coord
        .rewind_session(&sid, mid, RewindTarget::Both)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}
