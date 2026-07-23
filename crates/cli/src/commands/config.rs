use crate::args::GlobalArgs;
use crate::utils::{get_nested_value, load_config, set_nested_value};
use anyhow::{Context, Result};
use std::path::PathBuf;

fn config_path(global: &GlobalArgs) -> PathBuf {
    global
        .config
        .clone()
        .unwrap_or_else(kernel::config::Config::write_path)
}

pub fn show(global: &GlobalArgs) -> Result<()> {
    let config = load_config(global.config.as_ref())?;
    let toml_str = toml::to_string_pretty(&config)?;
    println!("{toml_str}");
    Ok(())
}

pub fn get(global: &GlobalArgs, key: &str) -> Result<()> {
    let config = load_config(global.config.as_ref())?;
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

pub fn set(global: &GlobalArgs, key: &str, value: String) -> Result<()> {
    let config_path = config_path(global);
    let mut config: toml::Table = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        content.parse().context("Invalid config TOML")?
    } else {
        toml::Table::new()
    };

    set_nested_value(&mut config, key, value)?;

    let content = toml::to_string_pretty(&config)?;
    kernel::config::Config::set_kernel_config_at(&config_path, &content)?;
    println!("Config saved to {}", config_path.display());
    Ok(())
}
