//! 主窗口「回到前台」的统一动作。
//!
//! 三个入口共用同一段编排（Dock 图标重开、从宠物窗打开会话、以及其他
//! 需要把主窗口拎出来的命令）：最小化/隐藏状态先还原，再聚焦。顺序只在
//! 这里定义一次——tao 的 `set_focus` 在窗口仍处于最小化或不可见时是
//! 静默 no-op，`show` 对最小化窗口也不生效，所以必须是
//! `unminimize` → `show` → `set_focus`。

use tauri::Manager;

/// 把主窗口从最小化/隐藏状态拉回前台并聚焦。
pub(crate) fn reveal_main(app_handle: &tauri::AppHandle) -> tauri::Result<()> {
    let Some(window) = app_handle.get_webview_window("main") else {
        return Err(tauri::Error::WindowNotFound);
    };
    tracing::debug!(
        minimized = window.is_minimized().unwrap_or(false),
        visible = window.is_visible().unwrap_or(false),
        "revealing main window"
    );
    window.unminimize()?;
    window.show()?;
    window.set_focus()?;
    Ok(())
}
