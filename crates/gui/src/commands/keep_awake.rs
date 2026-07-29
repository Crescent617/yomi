//! Keep-awake power assertions: prevent the OS from sleeping while Yomi
//! runs (the display may still turn off). The assertion is held by the GUI
//! process, which also hosts the managed daemon, so one toggle covers both.
//! Desktop OSes only — mobile builds get a stub that reports unsupported.

use crate::error::GuiError;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod imp {
    use std::sync::{Mutex, OnceLock};

    use keepawake::KeepAwake;

    /// The live power assertion (`None` = keep-awake off). Process exit
    /// releases the assertion automatically via the OS.
    static ASSERTION: OnceLock<Mutex<Option<KeepAwake>>> = OnceLock::new();

    fn slot() -> &'static Mutex<Option<KeepAwake>> {
        ASSERTION.get_or_init(|| Mutex::new(None))
    }

    pub(super) fn set(enabled: bool) -> Result<bool, String> {
        let mut guard = slot()
            .lock()
            .map_err(|e| format!("keep-awake lock poisoned: {e}"))?;
        if enabled == guard.is_some() {
            return Ok(enabled);
        }
        if enabled {
            // Machine stays awake (idle sleep + AC lid-close sleep), the
            // display is still allowed to turn off.
            let assertion = keepawake::Builder::default()
                .idle(true)
                .sleep(true)
                .reason("Yomi keep-awake is enabled")
                .app_name("Yomi")
                .app_reverse_domain("com.yomi.gui")
                .create()
                .map_err(|e| format!("failed to create power assertion: {e}"))?;
            *guard = Some(assertion);
        } else {
            guard.take(); // drop releases the assertion
        }
        Ok(enabled)
    }

    pub(super) fn get() -> bool {
        slot().lock().is_ok_and(|g| g.is_some())
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod imp {
    pub(super) fn set(_enabled: bool) -> Result<bool, String> {
        Err("keep-awake is not supported on this platform".to_string())
    }

    pub(super) fn get() -> bool {
        false
    }
}

/// Enable/disable the keep-awake power assertion; returns the state in
/// effect (idempotent — setting the current state is a no-op).
#[tauri::command(rename_all = "snake_case")]
pub fn set_keep_awake(enabled: bool) -> Result<bool, GuiError> {
    imp::set(enabled).map_err(GuiError::unknown)
}

/// Whether the keep-awake power assertion is currently held.
#[tauri::command(rename_all = "snake_case")]
pub fn get_keep_awake() -> bool {
    imp::get()
}

#[cfg(test)]
#[path = "keep_awake_test.rs"]
mod tests;
