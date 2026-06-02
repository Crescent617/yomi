use kernel::permissions::Level;
use kernel::types::{ContentBlock, SessionId};
use tauri::{AppHandle, Emitter, State};

use crate::error::GuiError;
use crate::state::AppState;

fn parse_level(s: &str) -> Result<Level, GuiError> {
    match s.to_lowercase().as_str() {
        "safe" => Ok(Level::Safe),
        "caution" => Ok(Level::Caution),
        "dangerous" => Ok(Level::Dangerous),
        _ => Err(GuiError::unknown(format!("Unknown permission level: {s}"))),
    }
}

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    session_id: String,
    content: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
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
    auto_approve_level: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id.clone());
    let level = parse_level(&auto_approve_level)?;
    let mut rx = match coord
        .subscribe_session_events(&sid, level)
        .await
    {
        Ok(rx) => rx,
        Err(_) => {
            coord
                .restore_session(&sid, level)
                .await
                .map_err(GuiError::kernel)?;
            coord
                .subscribe_session_events(&sid, level)
                .await
                .map_err(GuiError::kernel)?
        }
    };

    // Stop any existing event task BEFORE spawning the new one to avoid
    // stale tasks racing with the insert.
    state.stop_event_task(&session_id).await;

    let sid_cleanup = session_id.clone();
    let tasks_cleanup = state.event_tasks.clone();
    let handle = tauri::async_runtime::spawn(async move {
        while let Ok(event) = rx.recv().await {
            let payload = serde_json::json!({
                "sessionId": sid,
                "event": event,
            });
            let _ = app_handle.emit("kernel:event", payload);
        }
        // Remove our handle from the map when the channel closes so we don't leak.
        let mut map = tasks_cleanup.lock().await;
        map.remove(&sid_cleanup);
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
    let coord = state.coordinator.clone();
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

#[tauri::command]
pub async fn get_todos(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    match coord.get_todos(&sid).await.map_err(GuiError::kernel)? {
        Some(json_str) => {
            let parsed: serde_json::Value = serde_json::from_str(&json_str)
                .map_err(|e| GuiError::unknown(format!("Invalid todo JSON: {e}")))?;
            Ok(parsed)
        }
        None => Ok(serde_json::json!({ "todos": [] })),
    }
}

// ── Cancel ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn cancel_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord.cancel(&sid).await.map_err(GuiError::kernel)?;
    Ok(())
}

// ── Permission response ────────────────────────────────────────────────────

#[tauri::command]
pub async fn respond_permission(
    state: State<'_, AppState>,
    session_id: String,
    req_id: String,
    approved: bool,
    remember: bool,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .send_permission_response(&sid, &req_id, approved, remember)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Ask user response ────────────────────────────────────────────────────

#[tauri::command]
pub async fn respond_ask_user(
    state: State<'_, AppState>,
    session_id: String,
    req_id: String,
    answers: Vec<(String, String)>,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let response = kernel::tools::AskUserResponse {
        answers: answers.into_iter().collect(),
    };
    coord
        .send_ask_user_response(&sid, &req_id, response)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Compact session ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn compact_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .compact_session(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Set permission level ─────────────────────────────────────────────────

#[tauri::command]
pub async fn set_permission_level(
    state: State<'_, AppState>,
    session_id: String,
    level: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let level = parse_level(&level)?;
    coord
        .set_permission_level(&sid, level)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

// ── Goal mode ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn start_goal(
    state: State<'_, AppState>,
    session_id: String,
    description: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    let goal_state = kernel::goal::GoalState::new(description);
    coord
        .start_goal(&sid, goal_state)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}

#[tauri::command]
pub async fn stop_goal(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), GuiError> {
    let coord = state.coordinator.clone();
    let sid = SessionId(session_id);
    coord
        .stop_goal(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(())
}
