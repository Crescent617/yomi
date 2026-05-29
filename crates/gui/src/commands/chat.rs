use kernel::client::CoordinatorApi;
use kernel::types::{ContentBlock, SessionId};
use tauri::{AppHandle, Emitter, State};

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    content: String,
) -> Result<(), GuiError> {
    let coord = state.get_or_connect().await?;
    let sid = SessionId(session_id);
    let block = ContentBlock::Text { text: content };
    coord
        .send_message(&sid, vec![block])
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command]
pub async fn subscribe(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.get_or_connect().await?;
    let sid = SessionId(session_id.clone());
    let mut rx = coord
        .subscribe_session_events(&sid)
        .await
        .map_err(GuiError::kernel)?;

    // Stop any existing event task BEFORE spawning the new one to avoid
    // stale tasks racing with the insert.
    state.stop_event_task(&session_id).await;

    let sid = session_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let payload = serde_json::json!({
                "sessionId": sid,
                "event": event,
            });
            let _ = app_handle.emit("kernel:event", payload);
        }
    });

    {
        let mut tasks = state.event_tasks.lock().await;
        tasks.insert(session_id.clone(), handle);
    }

    {
        let mut guard = state.active_session.lock().await;
        *guard = Some(session_id);
    }
    Ok(())
}

#[tauri::command]
pub async fn unsubscribe(state: State<'_, AppState>, session_id: String) -> Result<(), GuiError> {
    state.stop_event_task(&session_id).await;
    {
        let mut guard = state.active_session.lock().await;
        if guard.as_ref() == Some(&session_id) {
            *guard = None;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn get_messages(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<serde_json::Value>, GuiError> {
    let coord = state.get_or_connect().await?;
    let sid = SessionId(session_id);
    let messages = coord
        .get_session_messages(&sid)
        .await
        .map_err(GuiError::kernel)?;
    let values: Vec<_> = messages
        .into_iter()
        .map(|m| serde_json::to_value(m).unwrap_or_default())
        .collect();
    Ok(values)
}
