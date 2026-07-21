use kernel::storage::{AddFavoriteInput, FavoriteAnswer};
use kernel::types::{MessageId, SessionId};
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn add_favorite(
    state: State<'_, AppState>,
    session_id: String,
    message_id: String,
    content: String,
    session_title: Option<String>,
    message_created_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<FavoriteAnswer, GuiError> {
    let coord = state.kernel.clone();
    let input = AddFavoriteInput {
        session_id: SessionId::from(session_id),
        message_id: MessageId::from(message_id),
        session_title,
        content,
        note: None,
        message_created_at,
    };
    coord.add_favorite(input).await.map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_favorite(
    state: State<'_, AppState>,
    favorite_id: String,
) -> Result<(), GuiError> {
    let coord = state.kernel.clone();
    coord
        .remove_favorite(&favorite_id)
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_favorite_by_message(
    state: State<'_, AppState>,
    session_id: String,
    message_id: String,
) -> Result<(), GuiError> {
    let coord = state.kernel.clone();
    coord
        .remove_favorite_by_message(&SessionId::from(session_id), &MessageId::from(message_id))
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_favorites(
    state: State<'_, AppState>,
    query: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<FavoriteAnswer>, GuiError> {
    let coord = state.kernel.clone();
    coord
        .list_favorites(query, limit.unwrap_or(200), offset.unwrap_or(0))
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn update_favorite_note(
    state: State<'_, AppState>,
    favorite_id: String,
    note: Option<String>,
) -> Result<(), GuiError> {
    let coord = state.kernel.clone();
    coord
        .update_favorite_note(&favorite_id, note)
        .await
        .map_err(GuiError::kernel)
}
