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

/// Print the JSON Schema of the config file, generated from the `Config`
/// type so it can never drift from the code. `docs/config-schema.json` is
/// the verbatim output of this command (enforced by a test).
///
/// `default` values are stripped: they are machine-dependent (e.g.
/// `data_dir` expands `~`) and verbose (the whole built-in system prompt),
/// and the effective defaults are always visible via `config show`.
pub fn schema() {
    println!("{}", schema_json_string());
}

pub(crate) fn schema_json_string() -> String {
    let mut schema = serde_json::to_value(schemars::schema_for!(kernel::config::Config))
        .expect("Config schema must serialize");
    strip_defaults(&mut schema);
    serde_json::to_string_pretty(&schema).expect("Config schema must serialize")
}

fn strip_defaults(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("default");
            for v in map.values_mut() {
                strip_defaults(v);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                strip_defaults(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "config_test.rs"]
mod tests;
