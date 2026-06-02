use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command]
pub async fn ping(_state: State<'_, AppState>) -> Result<bool, GuiError> {
    Ok(true)
}

#[tauri::command]
pub fn get_cwd() -> Result<String, GuiError> {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| GuiError::unknown(format!("Failed to get cwd: {e}")))
}

#[tauri::command]
pub async fn get_config(
    _state: State<'_, AppState>,
) -> Result<serde_json::Value, GuiError> {
    let working_dir = std::env::current_dir()
        .map_err(|e| GuiError::unknown(format!("Failed to get cwd: {e}")))?;
    let config_file = kernel::config::Config::discover_file();
    let mut config = if let Some(ref path) = config_file {
        kernel::config::Config::from_file(path)
            .map_err(|e| GuiError::unknown(format!("Failed to load config: {e}")))?
    } else {
        kernel::config::Config::default()
    };
    config.apply_env_overrides();
    config.finalize(&working_dir);

    let model = config.agent.model.model_id.clone();
    let context_window = config.agent.compactor.context_window;
    let provider = config.agent.model.provider.to_string();

    Ok(serde_json::json!({
        "model": model,
        "context_window": context_window,
        "provider": provider,
    }))
}

#[tauri::command]
pub async fn get_usage_summary(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.coordinator.clone();
    let summary = coord
        .get_usage_summary()
        .await
        .map_err(GuiError::kernel)?;
    Ok(serde_json::json!({
        "prompt_tokens": summary.prompt_tokens,
        "completion_tokens": summary.completion_tokens,
        "cached_tokens": summary.cached_tokens,
        "total_tokens": summary.total_tokens(),
        "request_count": summary.request_count,
    }))
}

#[tauri::command]
pub async fn get_daily_usage(
    state: State<'_, AppState>,
    days: i64,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.coordinator.clone();
    let daily = coord
        .get_daily_usage(days)
        .await
        .map_err(GuiError::kernel)?;
    let items: Vec<_> = daily
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "date": d.date,
                "prompt_tokens": d.prompt_tokens,
                "completion_tokens": d.completion_tokens,
                "cached_tokens": d.cached_tokens,
                "total_tokens": d.total_tokens(),
                "request_count": d.request_count,
                "models": d.models,
            })
        })
        .collect();
    Ok(serde_json::Value::Array(items))
}

#[tauri::command]
pub async fn get_session_usage(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.coordinator.clone();
    let sid = kernel::types::SessionId(session_id);
    let usage = coord
        .get_session_usage(&sid)
        .await
        .map_err(GuiError::kernel)?;
    Ok(serde_json::json!({
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "cached_tokens": usage.cached_tokens,
        "total_tokens": usage.total_tokens(),
        "request_count": usage.request_count,
    }))
}
