use crate::args::GlobalArgs;
use anyhow::Result;
use kernel::config::Config;
use std::path::PathBuf;
use std::sync::LazyLock;

/// Global debug mode flag, initialized from DEBUG=1 environment variable
pub static DEBUG_MODE: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("DEBUG").is_ok_and(|v| v == "1" || v.to_lowercase().contains('t'))
});

/// Load configuration from the specified path or discover default locations.
///
/// Resolution order:
/// 1. Explicit `config_path` (from `--config` / `-c` CLI arg)
/// 2. `YOMI_CONFIG` environment variable
/// 3. `~/.yomi/config.toml`
/// 4. Environment variables only
pub fn load_config(config_path: Option<&PathBuf>) -> Result<Config> {
    let mut config = if let Some(path) = config_path {
        Config::from_file(path)?
    } else {
        Config::discover_file()
            .map(|path| Config::from_file(&path))
            .transpose()?
            .unwrap_or_default()
    };

    config.inject_env()?;
    config.apply_env_overrides();
    config.finalize();
    config.validate()?;
    Ok(config)
}

/// Get a value from a JSON Value using dot notation (e.g., "`model.api_key`")
pub fn get_nested_value<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Option<&'a serde_json::Value> {
    let mut current = value;

    for part in key.split('.') {
        current = current.get(part)?;
    }

    Some(current)
}

/// Set a value in a TOML Table using dot notation (e.g., "`model.api_key`")
pub fn set_nested_value(table: &mut toml::Table, key: &str, value: String) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();

    let (last, init) = parts
        .split_last()
        .ok_or_else(|| anyhow::anyhow!("Empty config key"))?;

    let mut current: &mut toml::Table = table;

    for part in init {
        current = current
            .entry(*part)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("Cannot set nested value in non-table"))?;
    }

    let parsed_value = if let Ok(b) = value.parse::<bool>() {
        toml::Value::Boolean(b)
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else if let Ok(f) = value.parse::<f64>() {
        toml::Value::Float(f)
    } else {
        toml::Value::String(value)
    };

    current.insert((*last).to_string(), parsed_value);

    Ok(())
}

/// Get the data directory from global args
pub fn data_dir(global: &GlobalArgs) -> Result<PathBuf> {
    let config = load_config(global.config.as_ref())?;
    Ok(config.data_dir)
}

/// Open storage with configuration from global args
pub async fn open_storage(global: &GlobalArgs) -> Result<kernel::StorageSet> {
    let config = load_config(global.config.as_ref())?;
    kernel::StorageSet::open_with_config(&config.data_dir, &config)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to open storage: {e}"))
}

/// Resolve working directory from global args
/// Uses the provided dir or falls back to current directory
pub fn resolve_working_dir(global: &GlobalArgs) -> Result<PathBuf> {
    let dir = match global.dir.clone() {
        Some(d) => d,
        None => std::env::current_dir()?,
    };
    Ok(dir.canonicalize()?)
}

/// Maximum stdin size to prevent OOM (400KB)
const MAX_STDIN_SIZE: u64 = 400 * 1024;

/// Combine prompt and piped stdin: both present wraps stdin in a code fence.
pub fn combine_prompt_stdin(prompt: Option<String>, stdin: Option<String>) -> Option<String> {
    match (prompt, stdin) {
        (Some(p), Some(s)) => Some(format!("{p}\n\n```\n{s}\n```")),
        (Some(p), None) => Some(p),
        (None, Some(s)) => Some(s),
        (None, None) => None,
    }
}

/// Read piped stdin (non-TTY only), capped at 400KB. Returns None on TTY,
/// empty input, or read failure (logged).
pub async fn read_piped_stdin() -> Option<String> {
    use std::io::{IsTerminal, Read as _};

    if std::io::stdin().is_terminal() {
        return None;
    }
    // Read stdin in a blocking thread to avoid blocking the async runtime.
    match tokio::task::spawn_blocking(move || {
        let mut buffer = String::with_capacity((MAX_STDIN_SIZE as usize).min(8192));
        let mut stdin = std::io::stdin().take(MAX_STDIN_SIZE);
        match stdin.read_to_string(&mut buffer) {
            Ok(0) => None,
            Ok(n) => {
                // Only warn if we actually hit the limit (more data waiting).
                if n >= MAX_STDIN_SIZE as usize {
                    let mut extra = [0u8; 1];
                    if std::io::stdin().read(&mut extra).is_ok_and(|n| n > 0) {
                        tracing::warn!("Stdin truncated at {}KB limit", MAX_STDIN_SIZE / 1024);
                    }
                }
                let trimmed = buffer.trim_end().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }
            Err(e) => {
                tracing::warn!("Failed to read stdin: {e}");
                None
            }
        }
    })
    .await
    {
        Ok(content) => content,
        Err(e) => {
            tracing::warn!("Stdin reading task panicked: {e}");
            None
        }
    }
}
