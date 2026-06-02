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

    Ok(serde_json::json!({
        "model": model,
        "context_window": context_window,
    }))
}
