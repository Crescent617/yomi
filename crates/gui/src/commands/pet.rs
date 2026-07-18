use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::error::GuiError;
use crate::pet_runtime;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn get_pet_state(
    state: State<'_, AppState>,
) -> Result<crate::pet::PetSnapshot, GuiError> {
    let runtime = state.pet_runtime.lock().await;
    Ok(runtime.snapshot(std::time::Instant::now()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_pet_enabled(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    enabled: bool,
) -> Result<(), GuiError> {
    if enabled {
        pet_runtime::start_pet_runtime(&state, &app_handle).await?;
        pet_runtime::sync_running_sessions(&state).await?;
        let window = match app_handle.get_webview_window("pet") {
            Some(window) => window,
            None => {
                let builder =
                    WebviewWindowBuilder::new(&app_handle, "pet", WebviewUrl::App("/pet".into()))
                        .title("Yomi Pet")
                        .inner_size(152.0, 112.0)
                        .decorations(false)
                        .always_on_top(true)
                        .skip_taskbar(true)
                        .resizable(false)
                        .center()
                        .shadow(false)
                        .transparent(true);
                builder.build().map_err(|error| {
                    GuiError::unknown(format!("failed to create pet window: {error}"))
                })?
            }
        };
        window
            .set_background_color(Some(tauri::utils::config::Color(0, 0, 0, 0)))
            .map_err(|error| {
                GuiError::unknown(format!("failed to make pet window transparent: {error}"))
            })?;
        window
            .show()
            .map_err(|error| GuiError::unknown(format!("failed to show pet window: {error}")))?;
        state.set_pet_enabled(true);
        state.pet_runtime_notify.notify_one();
    } else {
        if let Some(window) = app_handle.get_webview_window("pet") {
            window.hide().map_err(|error| {
                GuiError::unknown(format!("failed to hide pet window: {error}"))
            })?;
        }
        state.set_pet_enabled(false);
    }
    Ok(())
}
