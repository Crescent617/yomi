use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

const DEBUG_CHUNK_BYTES: u64 = 256 * 1024;

#[tauri::command(rename_all = "snake_case")]
pub async fn list_gui_logs(state: State<'_, AppState>) -> Result<Vec<String>, GuiError> {
    let log_dir = state.gui_log_dir.clone();
    tauri::async_runtime::spawn_blocking(move || list_gui_log_files(&log_dir))
        .await
        .map_err(|error| GuiError::unknown(format!("Failed to list GUI logs: {error}")))?
}

#[tauri::command(rename_all = "snake_case")]
pub async fn read_session_jsonl(
    state: State<'_, AppState>,
    session_id: String,
    before_offset: Option<u64>,
    after_offset: Option<u64>,
) -> Result<kernel::client::SessionJsonlChunk, GuiError> {
    state
        .kernel
        .read_session_jsonl(
            &kernel::types::SessionId::from(session_id),
            before_offset,
            after_offset,
        )
        .await
        .map_err(|error| GuiError::unknown(format!("Failed to read session JSONL: {error}")))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn read_gui_log(
    state: State<'_, AppState>,
    file_name: String,
    before_offset: Option<u64>,
    after_offset: Option<u64>,
) -> Result<kernel::utils::file_chunk::FileChunk, GuiError> {
    if !is_gui_log_name(&file_name) {
        return Err(GuiError::unknown("Invalid GUI log file name"));
    }
    let path = state.gui_log_dir.join(file_name);
    let display_path = path.to_string_lossy().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        kernel::utils::file_chunk::read_utf8_file_chunk(
            &path,
            before_offset,
            after_offset,
            DEBUG_CHUNK_BYTES,
            false,
        )
    })
    .await
    .map_err(|error| GuiError::unknown(format!("Failed to read GUI log: {error}")))?
    .map_err(|error| GuiError::unknown(format!("Failed to read {display_path}: {error}")))
}

pub(crate) fn configured_log_dir() -> PathBuf {
    let mut config = kernel::config::Config::discover_file()
        .and_then(|path| kernel::config::Config::from_file(&path).ok())
        .unwrap_or_default();
    let _ = config.inject_env();
    config.apply_env_overrides();
    config.finalize();
    config.log_dir()
}

fn list_gui_log_files(log_dir: &Path) -> Result<Vec<String>, GuiError> {
    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_gui_log_name(name) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let Ok(link_metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if link_metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        let modified_at_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis() as u64);
        files.push((name.to_string(), modified_at_ms));
    }
    files.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.0.cmp(&left.0)));
    Ok(files.into_iter().map(|(name, _)| name).collect())
}

fn is_gui_log_name(name: &str) -> bool {
    let path = Path::new(name);
    if path.file_name().and_then(|part| part.to_str()) != Some(name)
        || path.extension().and_then(|extension| extension.to_str()) != Some("log")
    {
        return false;
    }
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    stem == "gui" || stem.starts_with("gui.")
}

#[cfg(test)]
#[path = "debug_test.rs"]
mod tests;
