use crate::args::GlobalArgs;
use crate::utils::{get_nested_value, set_nested_value};
use anyhow::{Context, Result};
use kernel::{config::Config, expand_tilde, DEFAULT_DATA_DIR};
use std::path::PathBuf;

fn config_path(global: &GlobalArgs) -> PathBuf {
    global
        .config
        .clone()
        .unwrap_or_else(|| expand_tilde(DEFAULT_DATA_DIR).join("config.toml"))
}

/// Load config from file if it exists, otherwise from environment
fn load_config_or_env(global: &GlobalArgs) -> Result<Config> {
    let path = config_path(global);
    if path.exists() {
        Config::from_file(&path)
    } else {
        Ok(Config::from_env())
    }
}

#[allow(clippy::needless_pass_by_value)]
pub fn show(global: GlobalArgs) -> Result<()> {
    let config = load_config_or_env(&global)?;
    let toml_str = toml::to_string_pretty(&config)?;
    println!("{toml_str}");
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
pub fn get(global: GlobalArgs, key: &str) -> Result<()> {
    let config = load_config_or_env(&global)?;
    let value = serde_json::to_value(&config)?;
    match get_nested_value(&value, key) {
        Some(v) => println!("{v}"),
        None => {
            eprintln!("Error: Config key '{key}' not found");
            std::process::exit(1);
        }
    }
    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
pub fn set(global: GlobalArgs, key: &str, value: String) -> Result<()> {
    let config_path = config_path(&global);
    let mut config: toml::Table = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        content.parse().context("Invalid config TOML")?
    } else {
        toml::Table::new()
    };

    set_nested_value(&mut config, key, value)?;

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&config_path, toml::to_string_pretty(&config)?)?;
    println!("Config saved to {}", config_path.display());
    Ok(())
}
