use tauri::State;

use crate::error::GuiError;
use crate::state::AppState;

#[tauri::command(rename_all = "snake_case")]
pub async fn ping(_state: State<'_, AppState>) -> Result<bool, GuiError> {
    Ok(true)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn read_asset(state: State<'_, AppState>, url: String) -> Result<Vec<u8>, GuiError> {
    let data_dir = state
        .data_dir
        .read()
        .map_err(|e| GuiError::unknown(format!("data_dir lock poisoned: {e}")))?
        .clone();
    if let Some(bytes) = kernel::utils::asset::read_asset(&url, &data_dir).await {
        Ok(bytes)
    } else {
        Err(GuiError::unknown(format!("Asset not found: {url}")))
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_daemon_status() -> Result<serde_json::Value, GuiError> {
    Ok(serde_json::json!({
        "managed": crate::daemon::is_managed().await,
    }))
}

/// Restart the daemon spawned by this GUI so it reloads the config file.
/// Fails when the GUI is connected to an externally started daemon.
#[tauri::command(rename_all = "snake_case")]
pub async fn restart_daemon(state: State<'_, AppState>) -> Result<(), GuiError> {
    let config = crate::daemon::restart_daemon()
        .await
        .map_err(|e| GuiError::unknown(format!("Failed to restart daemon: {e}")))?;
    if let Ok(mut data_dir) = state.data_dir.write() {
        *data_dir = config.data_dir;
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
pub async fn get_config_toml(_state: State<'_, AppState>) -> Result<serde_json::Value, GuiError> {
    let path = kernel::config::Config::discover_file();
    let (content, file_path) = match &path {
        Some(p) => {
            let c = std::fs::read_to_string(p)
                .map_err(|e| GuiError::unknown(format!("Failed to read config: {e}")))?;
            (c, p.to_string_lossy().to_string())
        }
        None => {
            let default_path = kernel::expand_tilde(kernel::DEFAULT_DATA_DIR).join("config.toml");
            (String::new(), default_path.to_string_lossy().to_string())
        }
    };
    Ok(serde_json::json!({
        "content": content,
        "path": file_path,
    }))
}

fn validate_config_toml(content: &str) -> Result<(), GuiError> {
    let config: kernel::config::Config =
        toml::from_str(content).map_err(|e| GuiError::unknown(format!("Invalid TOML: {e}")))?;

    let mut model_names = std::collections::HashSet::new();
    if config
        .models
        .iter()
        .any(|model| !model_names.insert(&model.name))
    {
        return Err(GuiError::unknown(
            "Invalid config: duplicate model name in [[models]]",
        ));
    }

    if !config.models.is_empty()
        && !config
            .models
            .iter()
            .any(|model| model.name == config.agent.default_model)
    {
        return Err(GuiError::unknown(
            "Invalid config: agent.default_model must match a [[models]] name",
        ));
    }

    Ok(())
}

fn resolve_config_write_target(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let mut target = path.to_path_buf();
    for _ in 0..40 {
        let metadata = match std::fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(target),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_symlink() {
            return Ok(target);
        }

        let link = std::fs::read_link(&target)?;
        target = if link.is_absolute() {
            link
        } else {
            target
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(link)
        };
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "too many symbolic links in config path",
    ))
}

fn atomic_write_config(file_path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;

    let file_path = resolve_config_write_target(file_path)?;
    let parent = file_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(content.as_bytes())?;
    temp_file.as_file().sync_all()?;
    temp_file.persist(file_path).map_err(|error| error.error)?;
    Ok(())
}

fn save_config_toml_to_path(file_path: &std::path::Path, content: &str) -> Result<(), GuiError> {
    validate_config_toml(content)?;
    atomic_write_config(file_path, content)
        .map_err(|e| GuiError::unknown(format!("Failed to write config: {e}")))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_config_toml(
    _state: State<'_, AppState>,
    content: String,
) -> Result<(), GuiError> {
    let path = kernel::config::Config::discover_file();
    let file_path = match path {
        Some(p) => p,
        None => {
            let default_path = kernel::expand_tilde(kernel::DEFAULT_DATA_DIR).join("config.toml");
            if let Some(parent) = default_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| GuiError::unknown(format!("Failed to create config dir: {e}")))?;
            }
            default_path
        }
    };

    save_config_toml_to_path(&file_path, &content)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_config(_state: State<'_, AppState>) -> Result<serde_json::Value, GuiError> {
    let config_file = kernel::config::Config::discover_file();
    let mut config = if let Some(ref path) = config_file {
        kernel::config::Config::from_file(path)
            .map_err(|e| GuiError::unknown(format!("Failed to load config: {e}")))?
    } else {
        kernel::config::Config::default()
    };
    config.apply_env_overrides();
    config.finalize();

    // `finalize()` is supposed to guarantee a valid default model; if it's
    // still missing, surface a real error instead of silently returning
    // empty model/provider and context_window = 0.
    let default_model = config.model().ok_or_else(|| {
        GuiError::unknown("invalid config: default_model does not match any entry in [models]")
    })?;
    let model = default_model.model_id.clone();
    let context_window = default_model.context_window;
    let provider = default_model.provider.to_string();

    let auto_approve = config.auto_approve.to_string().to_lowercase();
    let full_config = toml::to_string_pretty(&config)
        .map_err(|e| GuiError::unknown(format!("Failed to serialize config: {e}")))?;

    Ok(serde_json::json!({
        "model": model,
        "context_window": context_window,
        "provider": provider,
        "auto_approve": auto_approve,
        "full_config": full_config,
    }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_usage_summary(
    state: State<'_, AppState>,
    days: Option<i64>,
) -> Result<serde_json::Value, GuiError> {
    let coord = state.kernel.clone();
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
    let coord = state.kernel.clone();
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
    let coord = state.kernel.clone();
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
    let coord = state.kernel.clone();
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
pub async fn get_models(state: State<'_, AppState>) -> Result<serde_json::Value, GuiError> {
    let models = state.kernel.list_models().await.map_err(GuiError::kernel)?;
    Ok(serde_json::json!({ "models": models }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_session_model(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<String, GuiError> {
    let sid = kernel::SessionId::from(session_id);
    state
        .kernel
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
        .kernel
        .set_session_model(&sid, &key)
        .await
        .map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_in_browser(url: String) -> Result<(), GuiError> {
    tauri_plugin_opener::open_path(&url, None::<&str>)
        .map_err(|e| GuiError::unknown(format!("Failed to open browser: {e}")))?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn open_in_explorer(path: String) -> Result<(), GuiError> {
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|e| GuiError::unknown(format!("Failed to open explorer: {e}")))?;
    Ok(())
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

#[tauri::command(rename_all = "snake_case")]
pub async fn open_in_editor(path: String) -> Result<(), GuiError> {
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|e| GuiError::unknown(format!("Failed to open editor: {e}")))?;
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
