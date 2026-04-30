//! Yomi TUI - Terminal UI using tuirealm
//!
//! Component-based TUI framework with delta rendering and streaming markdown support.

use kernel::config::Config;
use std::sync::OnceLock;

pub mod app;
pub mod attr;
pub mod components;
pub mod id;
pub mod markdown_stream;
pub mod msg;
pub mod table;
pub mod theme;
pub mod utils;

// Re-export main entry point
pub use app::{run_tui, TuiResult};

// Global configuration
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Initialize global configuration (called once at startup)
pub fn init_config(config: Config) {
    CONFIG.set(config).expect("Config already initialized");
}

/// Get a reference to the global configuration
pub fn config() -> &'static Config {
    CONFIG.get().expect("Config not initialized")
}

// Re-export theme utilities
pub use theme::{
    chars, colors, current_theme, hex, presets, reset_theme, rgb, set_theme, spinner_char, Styles,
    ThemeConfig,
};
