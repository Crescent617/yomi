use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::commands::pet::pet_window_size;
use crate::error::GuiError;
use crate::state::AppState;

/// Companion panel size in logical pixels. The expanded window is the panel
/// stacked on top of (or below) the sprite, which keeps its screen anchor.
pub const PANEL_WIDTH: f64 = 380.0;
/// Panel height adapts to content within these bounds; `PANEL_HEIGHT` stays
/// as the maximum (and the fallback before the frontend reports content).
pub const MAX_PANEL_HEIGHT: f64 = 560.0;
pub const MIN_PANEL_HEIGHT: f64 = 240.0;
pub const DEFAULT_PANEL_HEIGHT: f64 = 360.0;
/// When the panel is open the sprite shrinks — the panel is the main act,
/// the pet only anchors it. Front- and backend share this single source via
/// `CompanionOpenDto.sprite_scale`.
pub const OPEN_SPRITE_SCALE_FACTOR: f64 = 0.7;

/// Where the panel sits relative to the sprite in the expanded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelPlacement {
    /// Panel above the sprite (sprite pinned to the frame's bottom edge).
    Above,
    /// Panel below the sprite (sprite pinned to the frame's top edge).
    Below,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct PanelFrame {
    pub position: LogicalPosition<f64>,
    pub size: LogicalSize<f64>,
    pub placement: PanelPlacement,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CompanionStateDto {
    pub enabled: bool,
    pub open: bool,
    pub pinned_session_id: Option<String>,
    pub hotkey: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct CompanionOpenDto {
    pub open: bool,
    pub placement: Option<PanelPlacement>,
    /// Sprite scale to render at after this transition (smaller when open,
    /// the configured pet scale when closed).
    pub sprite_scale: f64,
    /// Sprite center as a logical x offset inside the expanded frame. The
    /// frontend renders the sprite at this anchor (not window-centered), so
    /// a frame clamped against a screen edge does not shift the pet.
    /// `None` when closing.
    pub sprite_x: Option<f64>,
}

/// Breathing room between the expanded frame and the work-area edges.
const SCREEN_MARGIN: f64 = 8.0;

/// Pure layout math for the expanded companion frame. The sprite's screen
/// anchor (center-bottom for `above`, center-top for `below`) stays fixed
/// so the pet does not visually jump when the window grows or shrinks.
pub(crate) fn compute_panel_frame(
    sprite_size: LogicalSize<f64>,
    sprite_pos: LogicalPosition<f64>,
    work_area: Option<(LogicalPosition<f64>, LogicalSize<f64>)>,
    panel_height: f64,
) -> PanelFrame {
    let frame_w = PANEL_WIDTH.max(sprite_size.width);
    let frame_h = panel_height + sprite_size.height;
    let sprite_cx = sprite_pos.x + sprite_size.width / 2.0;
    let sprite_bottom = sprite_pos.y + sprite_size.height;

    let mut placement = PanelPlacement::Above;
    let mut x = sprite_cx - frame_w / 2.0;
    let mut y = sprite_bottom - frame_h;

    if let Some((wa_pos, wa_size)) = work_area {
        let wa_top = wa_pos.y + SCREEN_MARGIN;
        let wa_bottom = wa_pos.y + wa_size.height - SCREEN_MARGIN;
        let wa_left = wa_pos.x + SCREEN_MARGIN;
        let wa_right = wa_pos.x + wa_size.width - SCREEN_MARGIN;
        if y < wa_top {
            placement = PanelPlacement::Below;
            y = sprite_pos.y;
        }
        if y + frame_h > wa_bottom {
            y = (wa_bottom - frame_h).max(wa_top);
        }
        let max_x = wa_left + (wa_right - wa_left - frame_w).max(0.0);
        x = x.clamp(wa_left, max_x);
    }

    PanelFrame {
        position: LogicalPosition::new(x, y),
        size: LogicalSize::new(frame_w, frame_h),
        placement,
    }
}

/// Re-anchor an already-open frame after the panel height changes. The
/// placement chosen at open time is kept (no flip-flopping as content
/// grows); the sprite anchor stays exact.
pub(crate) fn reframe_open(
    sprite_size: LogicalSize<f64>,
    sprite_pos: LogicalPosition<f64>,
    placement: PanelPlacement,
    work_area: Option<(LogicalPosition<f64>, LogicalSize<f64>)>,
    panel_height: f64,
) -> PanelFrame {
    let frame_w = PANEL_WIDTH.max(sprite_size.width);
    let frame_h = panel_height + sprite_size.height;
    let sprite_cx = sprite_pos.x + sprite_size.width / 2.0;
    let sprite_bottom = sprite_pos.y + sprite_size.height;

    let mut x = sprite_cx - frame_w / 2.0;
    let mut y = match placement {
        PanelPlacement::Above => sprite_bottom - frame_h,
        PanelPlacement::Below => sprite_pos.y,
    };

    if let Some((wa_pos, wa_size)) = work_area {
        let wa_top = wa_pos.y + SCREEN_MARGIN;
        let wa_bottom = wa_pos.y + wa_size.height - SCREEN_MARGIN;
        let wa_left = wa_pos.x + SCREEN_MARGIN;
        let wa_right = wa_pos.x + wa_size.width - SCREEN_MARGIN;
        if y < wa_top {
            y = wa_top;
        }
        if y + frame_h > wa_bottom {
            y = (wa_bottom - frame_h).max(wa_top);
        }
        let max_x = wa_left + (wa_right - wa_left - frame_w).max(0.0);
        x = x.clamp(wa_left, max_x);
    }

    PanelFrame {
        position: LogicalPosition::new(x, y),
        size: LogicalSize::new(frame_w, frame_h),
        placement,
    }
}

async fn companion_dto(state: &AppState) -> CompanionStateDto {
    let companion = state.companion.lock().await;
    CompanionStateDto {
        enabled: companion.enabled,
        open: companion.open,
        pinned_session_id: state
            .companion_pinned_session
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        hotkey: companion.hotkey.clone(),
    }
}

async fn emit_companion_state(app_handle: &AppHandle, state: &AppState) {
    let _ = app_handle.emit_to("pet", "companion:state", companion_dto(state).await);
}

fn pet_window(app_handle: &AppHandle) -> Result<tauri::WebviewWindow, GuiError> {
    app_handle
        .get_webview_window("pet")
        .ok_or_else(|| GuiError::unknown("pet window is not available".to_string()))
}

fn pet_scale(state: &AppState) -> Result<f64, GuiError> {
    state
        .pet_scale
        .read()
        .map(|scale| *scale)
        .map_err(|error| GuiError::unknown(format!("pet_scale lock poisoned: {error}")))
}

type WorkArea = (LogicalPosition<f64>, LogicalSize<f64>);

fn current_work_area(window: &tauri::WebviewWindow) -> Result<Option<WorkArea>, GuiError> {
    let scale_factor = window
        .scale_factor()
        .map_err(|error| GuiError::unknown(format!("failed to read scale factor: {error}")))?;
    Ok(window.current_monitor().ok().flatten().map(|monitor| {
        let area = monitor.work_area();
        (
            area.position.to_logical::<f64>(scale_factor),
            area.size.to_logical::<f64>(scale_factor),
        )
    }))
}

/// Grow or shrink the pet window around its sprite anchor. Shared by the
/// command (sprite click / Esc) and the global hotkey path.
pub(crate) async fn apply_companion_open(
    app_handle: &AppHandle,
    state: &AppState,
    open: bool,
) -> Result<(), GuiError> {
    // Idempotent: the frontend's `open` flag only flips when the
    // `open_changed` event lands, so a double click (or hotkey racing a
    // click) can issue the same command twice within one IPC round trip.
    // A second `open` would capture the *expanded* frame as
    // `collapsed_frame` and teleport the sprite on close.
    if state.companion.lock().await.open == open {
        return Ok(());
    }
    let window = pet_window(app_handle)?;
    let scale = pet_scale(state)?;
    let open_sprite_scale = scale * OPEN_SPRITE_SCALE_FACTOR;
    let open_sprite_size = pet_window_size(open_sprite_scale);
    let collapsed_sprite_size = pet_window_size(scale);
    let mut dto = CompanionOpenDto {
        open,
        placement: None,
        sprite_scale: if open { open_sprite_scale } else { scale },
        sprite_x: None,
    };

    // The companion lock is held across the state mutation and the (sync)
    // window calls so a late `set_companion_panel_height` cannot reframe
    // the window in the middle of a close transition.
    let mut companion = state.companion.lock().await;
    if open {
        let scale_factor = window
            .scale_factor()
            .map_err(|error| GuiError::unknown(format!("failed to read scale factor: {error}")))?;
        let sprite_pos: LogicalPosition<f64> = window
            .outer_position()
            .map_err(|error| GuiError::unknown(format!("failed to read pet position: {error}")))?
            .to_logical(scale_factor);
        // The sprite keeps its center-bottom anchor but renders at the
        // smaller open scale inside the expanded frame.
        let sprite_cx = sprite_pos.x + collapsed_sprite_size.width / 2.0;
        let sprite_bottom = sprite_pos.y + collapsed_sprite_size.height;
        let open_sprite_pos = LogicalPosition::new(
            sprite_cx - open_sprite_size.width / 2.0,
            sprite_bottom - open_sprite_size.height,
        );
        let work_area = current_work_area(&window)?;
        let panel_height = companion
            .panel_height
            .unwrap_or(DEFAULT_PANEL_HEIGHT)
            .clamp(MIN_PANEL_HEIGHT, MAX_PANEL_HEIGHT);
        let frame = compute_panel_frame(open_sprite_size, open_sprite_pos, work_area, panel_height);
        companion.collapsed_frame = Some((sprite_pos, collapsed_sprite_size));
        companion.open_sprite = Some((open_sprite_pos, open_sprite_size, frame.placement));
        window.set_size(frame.size).map_err(|error| {
            GuiError::unknown(format!("failed to grow companion window: {error}"))
        })?;
        window.set_position(frame.position).map_err(|error| {
            GuiError::unknown(format!("failed to move companion window: {error}"))
        })?;
        window.set_focus().map_err(|error| {
            GuiError::unknown(format!("failed to focus companion window: {error}"))
        })?;
        dto.placement = Some(frame.placement);
        dto.sprite_x = Some(sprite_cx - frame.position.x);
    } else {
        companion.open_sprite = None;
        if let Some((position, size)) = companion.collapsed_frame.take() {
            window.set_size(size).map_err(|error| {
                GuiError::unknown(format!("failed to shrink companion window: {error}"))
            })?;
            window.set_position(position).map_err(|error| {
                GuiError::unknown(format!("failed to restore pet position: {error}"))
            })?;
        }
    }

    companion.open = open;
    drop(companion);
    let _ = app_handle.emit_to("pet", "companion:open_changed", dto);
    emit_companion_state(app_handle, state).await;
    Ok(())
}

/// Frontend reports the panel's natural content height; the window is
/// reframed around the open-time sprite anchor (placement kept), clamped to
/// the work area so neither sprite nor frame is pushed off-screen.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_companion_panel_height(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    height: f64,
) -> Result<(), GuiError> {
    if !height.is_finite() {
        return Err(GuiError::unknown(format!(
            "panel height must be finite, got {height}"
        )));
    }
    let requested = height.clamp(MIN_PANEL_HEIGHT, MAX_PANEL_HEIGHT);
    let window = pet_window(&app_handle)?;
    let work_area = current_work_area(&window)?;
    // The companion lock is held across validation and the (sync) window
    // calls, mutually exclusive with `apply_companion_open`: a report that
    // arrives mid-close errors out instead of re-expanding the window after
    // the collapsed frame was restored.
    let mut companion = state.companion.lock().await;
    let (sprite_pos, sprite_size, placement) = companion
        .open_sprite
        .ok_or_else(|| GuiError::unknown("companion panel is not open".to_string()))?;
    let available = work_area.map(|(wa_pos, wa_size)| match placement {
        PanelPlacement::Above => sprite_pos.y - (wa_pos.y + SCREEN_MARGIN),
        PanelPlacement::Below => {
            (wa_pos.y + wa_size.height - SCREEN_MARGIN) - (sprite_pos.y + sprite_size.height)
        }
    });
    let effective = match available {
        Some(limit) => requested.min(limit.max(120.0)),
        None => requested,
    };
    let frame = reframe_open(sprite_size, sprite_pos, placement, work_area, effective);
    window.set_size(frame.size).map_err(|error| {
        GuiError::unknown(format!("failed to resize companion window: {error}"))
    })?;
    window
        .set_position(frame.position)
        .map_err(|error| GuiError::unknown(format!("failed to move companion window: {error}")))?;
    companion.panel_height = Some(requested);
    Ok(())
}

/// Hotkey entry point: asks the pet window to toggle the panel. The
/// frontend drives the actual open/close so the collapse animation can play
/// before the window shrinks (a direct Rust-side close would cut it off).
pub async fn toggle(app_handle: &AppHandle) {
    let state = app_handle.state::<AppState>();
    let enabled = state.companion.lock().await.enabled;
    if !enabled {
        return;
    }
    let visible = pet_window(app_handle)
        .and_then(|window| {
            window
                .is_visible()
                .map_err(|error| GuiError::unknown(format!("failed to read visibility: {error}")))
        })
        .unwrap_or(false);
    if !visible {
        return;
    }
    let _ = app_handle.emit_to("pet", "companion:toggle_requested", ());
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_companion_state(
    state: State<'_, AppState>,
) -> Result<CompanionStateDto, GuiError> {
    Ok(companion_dto(&state).await)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_companion_enabled(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    enabled: bool,
) -> Result<(), GuiError> {
    let (was_enabled, was_open, hotkey) = {
        let companion = state.companion.lock().await;
        (companion.enabled, companion.open, companion.hotkey.clone())
    };
    if !enabled && was_open {
        apply_companion_open(&app_handle, &state, false).await?;
    }
    // The global shortcut is registered only while companion mode is
    // enabled — a disabled feature must not swallow a key combo system-wide.
    if enabled != was_enabled {
        if let Some(accelerator) = hotkey.as_deref() {
            if enabled {
                if let Err(error) = app_handle.global_shortcut().register(accelerator) {
                    // 注册失败不否决开关本身（热键可稍后在设置里重设）。
                    tracing::warn!(
                        "Failed to register companion hotkey \"{accelerator}\": {error}"
                    );
                }
            } else {
                let _ = app_handle.global_shortcut().unregister(accelerator);
            }
        }
    }
    state.companion.lock().await.enabled = enabled;
    emit_companion_state(&app_handle, &state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_companion_pinned_session(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    session_id: Option<String>,
) -> Result<(), GuiError> {
    *state
        .companion_pinned_session
        .write()
        .unwrap_or_else(|e| e.into_inner()) = session_id;
    emit_companion_state(&app_handle, &state).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_companion_open(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    open: bool,
) -> Result<(), GuiError> {
    let enabled = state.companion.lock().await.enabled;
    if open && !enabled {
        return Err(GuiError::unknown("companion mode is disabled".to_string()));
    }
    apply_companion_open(&app_handle, &state, open).await
}

/// Bring the main window to the front and hand it a session to activate.
/// The main window already listens for `app:open_session`; this adds the
/// window choreography (show/unminimize/focus) around the same event.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_session_in_main(
    app_handle: AppHandle,
    session_id: String,
) -> Result<(), GuiError> {
    let window = app_handle
        .get_webview_window("main")
        .ok_or_else(|| GuiError::unknown("main window is not available".to_string()))?;
    window
        .show()
        .map_err(|error| GuiError::unknown(format!("failed to show main window: {error}")))?;
    window
        .unminimize()
        .map_err(|error| GuiError::unknown(format!("failed to unminimize main window: {error}")))?;
    window
        .set_focus()
        .map_err(|error| GuiError::unknown(format!("failed to focus main window: {error}")))?;
    let _ = app_handle.emit_to(
        "main",
        "app:open_session",
        serde_json::json!({ "session_id": session_id }),
    );
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_companion_hotkey(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    accelerator: Option<String>,
) -> Result<(), GuiError> {
    let (enabled, previous) = {
        let mut companion = state.companion.lock().await;
        let previous = companion.hotkey.take();
        accelerator.clone_into(&mut companion.hotkey);
        (companion.enabled, previous)
    };
    // While disabled the desired hotkey is only stored; OS registration
    // happens on enable (see `set_companion_enabled`).
    if !enabled {
        emit_companion_state(&app_handle, &state).await;
        return Ok(());
    }
    if let Some(previous) = previous.as_deref() {
        let _ = app_handle.global_shortcut().unregister(previous);
    }
    if let Some(accelerator) = accelerator.as_deref() {
        if let Err(error) = app_handle.global_shortcut().register(accelerator) {
            // 新热键注册失败不能白丢旧热键——尽力回滚。
            let restored = previous.as_deref().and_then(|old| {
                app_handle
                    .global_shortcut()
                    .register(old)
                    .ok()
                    .map(|()| old.to_string())
            });
            state.companion.lock().await.hotkey = restored;
            emit_companion_state(&app_handle, &state).await;
            return Err(GuiError::unknown(format!(
                "failed to register companion hotkey \"{accelerator}\": {error}"
            )));
        }
    }
    emit_companion_state(&app_handle, &state).await;
    Ok(())
}

#[cfg(test)]
#[path = "companion_test.rs"]
mod tests;
