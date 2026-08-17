use std::sync::Arc;

use tauri::{Manager, State};

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn ping(_state: State<'_, AppState>) -> Result<bool, GuiError> {
    Ok(true)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn read_asset(state: State<'_, AppState>, url: String) -> Result<Vec<u8>, GuiError> {
    let kernel = state.kernel_snapshot();
    let source = kernel::utils::file_read::FileSource::Asset { url };
    let (bytes, _mime) = kernel::client::read_file_bytes(
        kernel.as_ref(),
        source,
        kernel::utils::image::MAX_IMAGE_SIZE,
    )
    .await
    .map_err(GuiError::kernel)?;
    Ok(bytes)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_daemon_status() -> Result<serde_json::Value, GuiError> {
    Ok(serde_json::json!({
        "managed": crate::daemon::is_managed().await,
    }))
}

fn connection_info_json(mode: &crate::state::ConnectionMode, managed: bool) -> serde_json::Value {
    match mode {
        crate::state::ConnectionMode::Local => serde_json::json!({
            "mode": "local",
            "addr": crate::daemon::socket_addr().to_string(),
            "managed": managed,
        }),
        crate::state::ConnectionMode::Remote(addr) => serde_json::json!({
            "mode": "remote",
            "addr": addr.to_string(),
            "managed": managed,
        }),
    }
}

/// Current daemon connection info (mode + address).
#[tauri::command(rename_all = "snake_case")]
pub async fn get_connection_info(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, GuiError> {
    let managed = crate::daemon::is_managed().await;
    Ok(connection_info_json(&state.connection_mode(), managed))
}

/// Switch the GUI to a remote daemon at `addr` (e.g. `wss://host:port`).
/// Validates connectivity before swapping; the previous connection stays
/// untouched on failure. The local daemon (if any) is deliberately left
/// running so switching back is instant and its cron jobs keep firing.
#[tauri::command(rename_all = "snake_case")]
pub async fn connect_remote(
    state: State<'_, AppState>,
    addr: String,
) -> Result<serde_json::Value, GuiError> {
    let _switch_guard = state.connection_switch.lock().await;
    let addr: kernel::transport::SocketAddr = addr
        .trim()
        .parse()
        .map_err(|e: String| GuiError::unknown(format!("Invalid socket address: {e}")))?;
    let remote = kernel::client::RemoteKernel::connect(&addr)
        .await
        .map_err(|e| GuiError::unknown(format!("Failed to connect to {addr}: {e}")))?;
    remote
        .check_ready()
        .await
        .map_err(|e| GuiError::unknown(format!("Daemon at {addr} is not ready: {e}")))?;
    state.swap_kernel(Arc::new(remote), crate::state::ConnectionMode::Remote(addr));
    let managed = crate::daemon::is_managed().await;
    Ok(connection_info_json(&state.connection_mode(), managed))
}

/// Leave remote mode and reconnect to the local daemon (connecting to an
/// existing one or spawning a background daemon if none is running).
#[tauri::command(rename_all = "snake_case")]
pub async fn disconnect_remote(state: State<'_, AppState>) -> Result<serde_json::Value, GuiError> {
    let _switch_guard = state.connection_switch.lock().await;
    let (kernel, data_dir) = crate::daemon::get_kernel()
        .await
        .map_err(GuiError::unknown)?;
    state.swap_kernel(kernel, crate::state::ConnectionMode::Local);
    if let Ok(mut guard) = state.data_dir.write() {
        *guard = data_dir;
    }
    let managed = crate::daemon::is_managed().await;
    Ok(connection_info_json(&state.connection_mode(), managed))
}

/// Restart the currently connected daemon through the unified kernel API.
#[tauri::command(rename_all = "snake_case")]
pub async fn restart_daemon(state: State<'_, AppState>) -> Result<(), GuiError> {
    let kernel = state.kernel_snapshot();
    kernel.restart().await.map_err(GuiError::kernel)?;
    let config = kernel.get_config().await.map_err(GuiError::kernel)?;
    if !config.full_config.is_empty() {
        let effective: kernel::config::Config = toml::from_str(&config.full_config)
            .map_err(|e| GuiError::unknown(format!("Failed to parse effective config: {e}")))?;
        if let Ok(mut data_dir) = state.data_dir.write() {
            *data_dir = effective.data_dir;
        }
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub fn get_cwd() -> Result<String, GuiError> {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| GuiError::unknown(format!("Failed to get cwd: {e}")))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_config_toml(state: State<'_, AppState>) -> Result<serde_json::Value, GuiError> {
    let config = state
        .kernel_snapshot()
        .get_config()
        .await
        .map_err(GuiError::kernel)?;
    serde_json::to_value(config).map_err(|e| GuiError::unknown(e.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_config_toml(state: State<'_, AppState>, content: String) -> Result<(), GuiError> {
    state
        .kernel_snapshot()
        .set_config(content)
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_config(state: State<'_, AppState>) -> Result<serde_json::Value, GuiError> {
    let kernel_config = state
        .kernel_snapshot()
        .get_config()
        .await
        .map_err(GuiError::kernel)?;
    let config: kernel::config::Config = if kernel_config.full_config.is_empty() {
        return Err(GuiError::unknown(
            "Invalid config: saved config cannot be applied",
        ));
    } else {
        toml::from_str(&kernel_config.full_config)
            .map_err(|e| GuiError::unknown(format!("Failed to parse effective config: {e}")))?
    };

    let default_model = config.model().ok_or_else(|| {
        GuiError::unknown("invalid config: default_model does not match any entry in [models]")
    })?;
    let model = default_model.model_id.clone();
    let context_window = default_model.context_window;
    let provider = default_model.provider.to_string();
    let auto_approve = config.auto_approve.to_string().to_lowercase();

    Ok(serde_json::json!({
        "model": model,
        "context_window": context_window,
        "provider": provider,
        "auto_approve": auto_approve,
        "full_config": kernel_config.full_config,
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_usage_summary(
    state: State<'_, AppState>,
    days: Option<i64>,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.kernel_snapshot();
    let days = days.unwrap_or(365);
    let summary = coord
        .get_usage_summary(days)
        .await
        .map_err(GuiError::kernel)?;
    Ok(serde_json::json!({
        "prompt_tokens": summary.prompt_tokens,
        "completion_tokens": summary.completion_tokens,
        "cached_tokens": summary.cached_tokens,
        "request_count": summary.request_count,
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_daily_usage(
    state: State<'_, AppState>,
    days: i64,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.kernel_snapshot();
    tracing::info!("get_daily_usage called with days={}", days);
    let daily = coord
        .get_daily_usage(days)
        .await
        .map_err(GuiError::kernel)?;
    tracing::info!("get_daily_usage returned {} days", daily.len());
    let items: Vec<_> = daily
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "date": d.date,
                "prompt_tokens": d.prompt_tokens,
                "completion_tokens": d.completion_tokens,
                "cached_tokens": d.cached_tokens,
                "request_count": d.request_count,
                "models": d.models,
            })
        })
        .collect();
    Ok(serde_json::Value::Array(items))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_model_usage(
    state: State<'_, AppState>,
    days: Option<i64>,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.kernel_snapshot();
    let days = days.unwrap_or(365);
    let usage = coord
        .get_model_usage(days)
        .await
        .map_err(GuiError::kernel)?;
    serde_json::to_value(usage).map_err(|e| GuiError::unknown(e.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_today_model_usage(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.kernel_snapshot();
    // 本地时区今日零点 -> UTC，与 daily_summary 的 localtime 口径一致
    let local_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .ok_or_else(|| GuiError::unknown("failed to compute local midnight".to_string()))?;
    let usage = coord
        .get_model_usage_since(local_start.with_timezone(&chrono::Utc))
        .await
        .map_err(GuiError::kernel)?;
    serde_json::to_value(usage).map_err(|e| GuiError::unknown(e.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_usage_records(
    state: State<'_, AppState>,
    before_id: Option<String>,
    limit: Option<usize>,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.kernel_snapshot();
    let records = coord
        .get_usage_records(before_id.as_deref(), limit.unwrap_or(50))
        .await
        .map_err(GuiError::kernel)?;
    serde_json::to_value(records).map_err(|e| GuiError::unknown(e.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_models(state: State<'_, AppState>) -> Result<serde_json::Value, GuiError> {
    let models = state
        .kernel_snapshot()
        .list_models()
        .await
        .map_err(GuiError::kernel)?;
    Ok(serde_json::json!({ "models": models }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_session_model(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<String, GuiError> {
    let sid = kernel::SessionId::from(session_id);
    state
        .kernel_snapshot()
        .get_session_model(&sid)
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_session_model(
    state: State<'_, AppState>,
    session_id: String,
    key: String,
) -> Result<(), GuiError> {
    let sid = kernel::SessionId::from(session_id);
    state
        .kernel_snapshot()
        .set_session_model(&sid, &key)
        .await
        .map_err(GuiError::kernel)
}

/// Open a URL or file path in its default application.
/// Uses `open_url` for web URLs (http/https/mailto), `open_path` for everything else.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_default(target: String) -> Result<(), GuiError> {
    let result = if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
    {
        tauri_plugin_opener::open_url(&target, None::<&str>)
    } else {
        tauri_plugin_opener::open_path(&target, None::<&str>)
    };
    result.map_err(|e| GuiError::unknown(format!("Failed to open: {e}")))?;
    Ok(())
}

/// Resolve a declared attachment path against the session workspace
/// (`kernel::utils::attachments::resolve_attachment`), mapping every
/// failure to the same user-facing error. Local-mode fast path of
/// [`open_attachment`].
async fn resolve_attachment_arg(
    base_dir: Option<String>,
    path: &str,
) -> Result<std::path::PathBuf, GuiError> {
    let base = base_dir.filter(|d| !d.is_empty());
    kernel::utils::attachments::resolve_attachment(base.as_deref().map(std::path::Path::new), path)
        .await
        .ok_or_else(|| {
            GuiError::unknown(format!(
                "attachment unavailable: {path} (missing, not a file, or outside the workspace)"
            ))
        })
}

/// Open a declared attachment file (from a `<yomi_attachments>` block in an
/// assistant message). Resolution follows the same rules as channel
/// delivery (`kernel::utils::attachments::resolve_attachment`) — applied
/// on the daemon's host, so both connection modes behave the same.
///
/// Local mode opens the file in place (edits land on the real file).
/// Remote mode fetches the bytes over the wire into a local content-keyed
/// cache and opens that copy — edits do NOT propagate back to the daemon.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_attachment(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    base_dir: Option<String>,
    path: String,
) -> Result<(), GuiError> {
    let target = match state.connection_mode() {
        crate::state::ConnectionMode::Local => resolve_attachment_arg(base_dir, &path).await?,
        crate::state::ConnectionMode::Remote(addr) => {
            fetch_remote_attachment(&app, &state.kernel_snapshot(), &addr, base_dir, &path).await?
        }
    };
    tauri_plugin_opener::open_path(&target, None::<&str>)
        .map_err(|e| GuiError::unknown(format!("Failed to open: {e}")))?;
    Ok(())
}

/// Fetch a remote attachment into the local cache and return the cached
/// copy's path. The daemon stats the file first (a `limit = 0` read, which
/// is also where unsafe paths are rejected); the cache entry name encodes
/// size + mtime, so a changed file invalidates naturally and a re-click on
/// an unchanged file opens instantly.
async fn fetch_remote_attachment(
    app: &tauri::AppHandle,
    kernel: &Arc<dyn kernel::client::KernelApi>,
    addr: &kernel::transport::SocketAddr,
    base_dir: Option<String>,
    path: &str,
) -> Result<std::path::PathBuf, GuiError> {
    let source = kernel::utils::file_read::FileSource::Attachment {
        base_dir: base_dir.filter(|d| !d.is_empty()),
        path: path.to_string(),
    };
    let meta = kernel
        .read_file(source.clone(), None, Some(0))
        .await
        .map_err(GuiError::kernel)?;

    let cache_root = app
        .path()
        .app_cache_dir()
        .map_err(|e| GuiError::unknown(format!("locate app cache dir: {e}")))?;
    let dir = remote_cache_dir(&cache_root, addr, path);
    let target = dir.join(cache_entry_name(meta.file_size, meta.mtime_ms, path));
    if !target.exists() {
        download_remote_file(kernel, &source, meta.file_size, &dir, &target).await?;
    }
    Ok(target)
}

/// Per-attachment cache directory for remote downloads:
/// `{cache}/remote-attachments/{hash(daemon + path)}/`.
fn remote_cache_dir(
    cache_root: &std::path::Path,
    addr: &kernel::transport::SocketAddr,
    path: &str,
) -> std::path::PathBuf {
    let key = blake3::hash(format!("{addr}\0{path}").as_bytes()).to_hex();
    cache_root
        .join("remote-attachments")
        .join(&key.as_str()[..16])
}

/// Cache entry name encoding content identity: a file whose size or mtime
/// changed gets a new name, so stale copies never open.
fn cache_entry_name(file_size: u64, mtime_ms: u64, path: &str) -> String {
    let basename = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("file");
    format!("{mtime_ms}-{file_size}-{basename}")
}

/// Stream `source` into `target` chunk by chunk (via a temp file in the
/// same directory, renamed into place on completion), replacing stale
/// versions of the same remote attachment.
async fn download_remote_file(
    kernel: &Arc<dyn kernel::client::KernelApi>,
    source: &kernel::utils::file_read::FileSource,
    file_size: u64,
    dir: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), GuiError> {
    use base64::Engine as _;
    use tokio::io::AsyncWriteExt;

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| GuiError::unknown(format!("create attachment cache: {e}")))?;
    // Replace stale versions of this attachment.
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let _ = tokio::fs::remove_file(entry.path()).await;
    }

    let tmp = dir.join(".download");
    let result: Result<(), GuiError> = async {
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut offset = 0u64;
        while offset < file_size {
            let chunk = kernel
                .read_file(source.clone(), Some(offset), None)
                .await
                .map_err(GuiError::kernel)?;
            if chunk.end_offset <= offset {
                return Err(GuiError::unknown(format!(
                    "download stalled at {offset}/{file_size} bytes"
                )));
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(&chunk.data_base64)
                .map_err(|e| GuiError::unknown(format!("decode file chunk: {e}")))?;
            file.write_all(&data).await?;
            offset = chunk.end_offset;
        }
        file.flush().await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result?;
    tokio::fs::rename(&tmp, target)
        .await
        .map_err(|e| GuiError::unknown(format!("store attachment: {e}")))?;
    Ok(())
}

/// Inline image payload for the attachment gallery.
#[derive(serde::Serialize)]
pub struct AttachmentImage {
    pub data_base64: String,
    pub mime: String,
}

/// Largest image read for inline display (rejects accidental huge reads).
const MAX_INLINE_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Read an image attachment for inline display. Bytes come from the daemon
/// over `KernelApi::read_file`, so local and remote mode behave the same;
/// non-images and oversized files are rejected (the frontend falls back to
/// a plain chip).
#[tauri::command(rename_all = "snake_case")]
pub async fn read_attachment_image(
    state: State<'_, AppState>,
    base_dir: Option<String>,
    path: String,
) -> Result<AttachmentImage, GuiError> {
    use base64::Engine as _;

    let kernel = state.kernel_snapshot();
    let source = kernel::utils::file_read::FileSource::Attachment {
        base_dir: base_dir.filter(|d| !d.is_empty()),
        path: path.clone(),
    };
    let (bytes, mime) =
        kernel::client::read_file_bytes(kernel.as_ref(), source, MAX_INLINE_IMAGE_BYTES)
            .await
            .map_err(GuiError::kernel)?;
    if !mime.starts_with("image/") {
        return Err(GuiError::unknown(format!("not an image: {path}")));
    }
    Ok(AttachmentImage {
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_in_vscode(path: String) -> Result<(), GuiError> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-a", "Visual Studio Code", &path])
            .spawn()
            .map_err(|e| GuiError::unknown(format!("Failed to open VS Code: {e}")))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::process::Command::new("code")
            .arg(&path)
            .spawn()
            .map_err(|e| GuiError::unknown(format!("Failed to open VS Code: {e}")))?;
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_in_zed(path: String) -> Result<(), GuiError> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-a", "Zed", &path])
            .spawn()
            .map_err(|e| GuiError::unknown(format!("Failed to open Zed: {e}")))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::process::Command::new("zed")
            .arg(&path)
            .spawn()
            .map_err(|e| GuiError::unknown(format!("Failed to open Zed: {e}")))?;
    }
    Ok(())
}

/// Walk up from `path` to find a `.git` directory or file (worktree).
/// If `start` is a file, begins from its parent directory.
fn find_git_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut current = if start.is_file() {
        start.parent()
    } else {
        Some(start)
    };
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Run a git command inside `repo_root` and return trimmed stdout.
fn git_stdout(repo_root: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .env("LC_ALL", "C")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_git_diff_summary(
    path: String,
    staged: bool,
) -> Result<serde_json::Value, GuiError> {
    let start = std::path::Path::new(&path);
    let Some(repo_root) = find_git_root(start) else {
        return Ok(serde_json::json!(null));
    };

    let status_args = if staged {
        &["diff", "--cached", "--name-status", "--no-renames"][..]
    } else {
        &["diff", "--name-status", "--no-renames"][..]
    };

    let status = git_stdout(&repo_root, status_args);
    let Some(status) = status else {
        return Ok(serde_json::json!(null));
    };

    let mut files = Vec::new();
    for line in status.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let status_char = parts[0].chars().next().unwrap_or('M');
        let file_path = parts[1];

        files.push(serde_json::json!({
            "path": file_path,
            "status": match status_char {
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                _ => "modified",
            },
        }));
    }

    Ok(serde_json::json!(files))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_git_file_diff_raw(
    path: String,
    file_path: String,
    staged: bool,
) -> Result<Option<String>, GuiError> {
    let start = std::path::Path::new(&path);
    let Some(repo_root) = find_git_root(start) else {
        return Ok(None);
    };

    let args: Vec<&str> = if staged {
        vec!["diff", "--cached", "--", &file_path]
    } else {
        vec!["diff", "--", &file_path]
    };

    Ok(git_stdout(&repo_root, &args))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_git_info(path: String) -> Result<serde_json::Value, GuiError> {
    let start = std::path::Path::new(&path);
    let Some(repo_root) = find_git_root(start) else {
        return Ok(serde_json::json!(null));
    };

    // Graceful fallback when git is not installed.
    if git_stdout(&repo_root, &["--version"]).is_none() {
        return Ok(serde_json::json!(null));
    }

    let branch = git_stdout(&repo_root, &["rev-parse", "--abbrev-ref", "HEAD"]);

    // Line-level stats via --shortstat
    let parse_shortstat = |out: Option<String>| -> (usize, usize) {
        let mut insertions = 0;
        let mut deletions = 0;
        if let Some(text) = out {
            let text = text.trim();
            if !text.is_empty() {
                for part in text.split(',') {
                    let part = part.trim();
                    if part.contains("insertion") {
                        if let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok())
                        {
                            insertions = n;
                        }
                    } else if part.contains("deletion") {
                        if let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok())
                        {
                            deletions = n;
                        }
                    }
                }
            }
        }
        (insertions, deletions)
    };

    let unstaged = git_stdout(&repo_root, &["diff", "--shortstat"]);
    let (unstaged_add, unstaged_del) = parse_shortstat(unstaged);
    let staged = git_stdout(&repo_root, &["diff", "--cached", "--shortstat"]);
    let (staged_add, staged_del) = parse_shortstat(staged);

    let added_lines = unstaged_add + staged_add;
    let deleted_lines = unstaged_del + staged_del;

    // Untracked file count (still file-level)
    let mut untracked = 0;
    let status = git_stdout(&repo_root, &["status", "--porcelain", "-uall"]);
    if let Some(ref s) = status {
        for line in s.lines() {
            if line.len() >= 2 && &line[..2] == "??" {
                untracked += 1;
            }
        }
    }

    Ok(serde_json::json!({
        "branch": branch,
        "added_lines": added_lines,
        "deleted_lines": deleted_lines,
        "untracked": untracked,
        "repo_root": repo_root.to_string_lossy().to_string(),
    }))
}

#[cfg(test)]
#[path = "system_test.rs"]
mod tests;
