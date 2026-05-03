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
pub use app::{run_tui, FeatureGates, OnInputHook, TuiResult};

// Global configuration
static CONFIG: OnceLock<Config> = OnceLock::new();

// Global feature gates
static FEATURE_GATES: OnceLock<FeatureGates> = OnceLock::new();

/// Initialize global configuration (called once at startup)
pub fn init_config(config: Config, feature_gates: FeatureGates) {
    CONFIG.set(config).expect("Config already initialized");
    FEATURE_GATES
        .set(feature_gates)
        .expect("Feature gates already initialized");
}

/// Get a reference to the global configuration
pub fn config() -> &'static Config {
    CONFIG.get().expect("Config not initialized")
}

/// Get a reference to the global feature gates
pub fn feature_gates() -> &'static FeatureGates {
    FEATURE_GATES.get().expect("Feature gates not initialized")
}

// Re-export theme utilities
pub use theme::{
    chars, colors, current_theme, hex, presets, reset_theme, rgb, set_theme, spinner_char, Styles,
    ThemeConfig,
};
