//! Yomi TUI - Terminal UI using tuirealm
//!
//! Component-based TUI framework with delta rendering and streaming markdown support.

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

// Re-export theme utilities
pub use theme::{
    chars, colors, current_theme, hex, presets, reset_theme, rgb, set_theme, spinner_char, Styles,
    ThemeConfig,
};
