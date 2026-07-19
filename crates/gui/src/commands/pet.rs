use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::error::GuiError;
use crate::pet_pack::{self, PetPack};
use crate::pet_runtime;
use crate::state::AppState;

/// The pet window fits exactly one Codex Pets sprite cell (192 x 208) times
/// the configured scale.
const PET_CELL_WIDTH: f64 = 192.0;
const PET_CELL_HEIGHT: f64 = 208.0;
/// Allowed pet scale range; the settings UI offers presets inside it.
const MIN_PET_SCALE: f64 = 0.5;
const MAX_PET_SCALE: f64 = 3.0;

fn pet_window_size(scale: f64) -> tauri::LogicalSize<f64> {
    tauri::LogicalSize::new(PET_CELL_WIDTH * scale, PET_CELL_HEIGHT * scale)
}

fn normalize_pet_scale(scale: f64) -> Option<f64> {
    (scale.is_finite() && (MIN_PET_SCALE..=MAX_PET_SCALE).contains(&scale)).then_some(scale)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_pet_scale(state: State<'_, AppState>) -> Result<f64, GuiError> {
    pet_scale(&state)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_pet_scale(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    scale: f64,
) -> Result<(), GuiError> {
    let scale = normalize_pet_scale(scale).ok_or_else(|| {
        GuiError::unknown(format!(
            "pet scale must be finite and within {MIN_PET_SCALE}..={MAX_PET_SCALE}, got {scale}"
        ))
    })?;
    *state
        .pet_scale
        .write()
        .map_err(|error| GuiError::unknown(format!("pet_scale lock poisoned: {error}")))? = scale;
    if let Some(window) = app_handle.get_webview_window("pet") {
        window
            .set_size(pet_window_size(scale))
            .map_err(|error| GuiError::unknown(format!("failed to size pet window: {error}")))?;
    }
    let _ = app_handle.emit_to("pet", "pet:scale_changed", scale);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_pet_state(
    state: State<'_, AppState>,
) -> Result<crate::pet::PetSnapshot, GuiError> {
    let runtime = state.pet_runtime.lock().await;
    Ok(runtime.snapshot(std::time::Instant::now()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_pet_packs() -> Result<Vec<PetPack>, GuiError> {
    pet_pack::discover_pet_packs().map_err(pet_pack_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn select_pet_pack(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    pet_id: Option<String>,
) -> Result<(), GuiError> {
    let selected = match pet_id.as_deref() {
        Some(id) => Some(
            pet_pack::validate_pet_pack(id)
                .map_err(pet_pack_error)?
                .pack,
        ),
        None => None,
    };

    *state
        .selected_pet_id
        .write()
        .map_err(|error| GuiError::unknown(format!("selected_pet_id lock poisoned: {error}")))? =
        pet_id;
    let _ = app_handle.emit_to("pet", "pet:pack_changed", selected);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_selected_pet_pack(
    state: State<'_, AppState>,
) -> Result<Option<PetPack>, GuiError> {
    let selected_id = selected_pet_id(&state)?;
    selected_id
        .as_deref()
        .map(|id| {
            pet_pack::validate_pet_pack(id)
                .map(|validated| validated.pack)
                .map_err(pet_pack_error)
        })
        .transpose()
}

#[tauri::command(rename_all = "snake_case")]
pub async fn read_selected_pet_spritesheet(
    state: State<'_, AppState>,
    window: WebviewWindow,
    pet_id: String,
    sprite_version_number: u32,
) -> Result<tauri::ipc::Response, GuiError> {
    if window.label() != "pet" {
        return Err(GuiError::unknown(
            "pet spritesheet may only be read by the pet window",
        ));
    }
    let selected_id =
        selected_pet_id(&state)?.ok_or_else(|| GuiError::unknown("no pet pack is selected"))?;
    if selected_id != pet_id {
        return Err(GuiError::unknown("selected pet pack changed while loading"));
    }
    let bytes =
        pet_pack::read_pet_spritesheet(&pet_id, sprite_version_number).map_err(pet_pack_error)?;
    if selected_pet_id(&state)?.as_deref() != Some(pet_id.as_str()) {
        return Err(GuiError::unknown("selected pet pack changed while loading"));
    }
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_pet_enabled(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    enabled: bool,
) -> Result<(), GuiError> {
    if enabled {
        let selected_id = selected_pet_id(&state)?
            .ok_or_else(|| GuiError::unknown("select a valid pet pack before enabling the pet"))?;
        pet_pack::validate_pet_pack(&selected_id).map_err(pet_pack_error)?;

        pet_runtime::start_pet_runtime(&state, &app_handle).await?;
        pet_runtime::sync_running_sessions(&state).await?;
        let scale = pet_scale(&state)?;
        let window = match app_handle.get_webview_window("pet") {
            Some(window) => {
                window.set_size(pet_window_size(scale)).map_err(|error| {
                    GuiError::unknown(format!("failed to resize existing pet window: {error}"))
                })?;
                window
            }
            None => {
                let builder =
                    WebviewWindowBuilder::new(&app_handle, "pet", WebviewUrl::App("/pet".into()))
                        .title("Yomi Pet")
                        .inner_size(PET_CELL_WIDTH * scale, PET_CELL_HEIGHT * scale)
                        .decorations(false)
                        .always_on_top(true)
                        .skip_taskbar(true)
                        .resizable(false)
                        .center()
                        .shadow(false)
                        // Transparent WebKitGTK windows retain stale animation frames on Linux.
                        .transparent(!cfg!(target_os = "linux"))
                        .background_color(pet_background_color());
                builder.build().map_err(|error| {
                    GuiError::unknown(format!("failed to create pet window: {error}"))
                })?
            }
        };
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

fn pet_background_color() -> tauri::utils::config::Color {
    if cfg!(target_os = "linux") {
        tauri::utils::config::Color(255, 255, 255, 255)
    } else {
        tauri::utils::config::Color(0, 0, 0, 0)
    }
}

fn selected_pet_id(state: &AppState) -> Result<Option<String>, GuiError> {
    state
        .selected_pet_id
        .read()
        .map(|selected| selected.clone())
        .map_err(|error| GuiError::unknown(format!("selected_pet_id lock poisoned: {error}")))
}

fn pet_scale(state: &AppState) -> Result<f64, GuiError> {
    state
        .pet_scale
        .read()
        .map(|scale| *scale)
        .map_err(|error| GuiError::unknown(format!("pet_scale lock poisoned: {error}")))
}

#[allow(clippy::needless_pass_by_value)]
fn pet_pack_error(error: pet_pack::PetPackError) -> GuiError {
    GuiError::unknown(format!("invalid pet pack: {error}"))
}

#[cfg(test)]
#[path = "pet_test.rs"]
mod tests;
