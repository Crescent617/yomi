//! Nekoclaw TUI - Terminal UI (stdout style, no alt screen)

pub mod app;
pub mod model;
pub mod theme;

// Keep old modules for compatibility
pub mod fold;
pub mod input;
pub mod markdown;
pub mod render;

pub use app::App;
pub use model::{ChatMessage, MessageId, Role, StreamingState, ToolCall};
pub use theme::{
    chars, colors, current_theme, hex, presets, reset_theme, rgb, set_theme, spinner_char, Styles,
    ThemeConfig,
};
