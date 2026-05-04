use crate::args::GlobalArgs;
use anyhow::Result;
use kernel::{config::Config, expand_tilde, DEFAULT_DATA_DIR};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Global debug mode flag, initialized from DEBUG=1 environment variable
pub static DEBUG_MODE: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("DEBUG").is_ok_and(|v| v == "1" || v.to_lowercase().contains('t'))
});

/// Load configuration from the specified path or search default locations
pub fn load_config(config_path: Option<&PathBuf>, working_dir: &Path) -> Result<Config> {
    let mut config = if let Some(path) = config_path {
        Config::from_file(path)?
    } else {
        let default_paths = [expand_tilde(DEFAULT_DATA_DIR).join("config.toml")];
        let mut loaded = None;
        for path in &default_paths {
            if path.exists() {
                loaded = Some(Config::from_file(path)?);
                break;
            }
        }
        loaded.unwrap_or_else(Config::from_env)
    };

    config.apply_env_overrides();
    config.finalize(working_dir);
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
    let working_dir = global
        .dir
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let config = load_config(global.config.as_ref(), &working_dir)?;
    Ok(config.data_dir)
}
