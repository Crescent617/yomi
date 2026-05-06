//! Unified chat view component
//!
//! Displays chat history + streaming message in a single scrollable view.

mod core;
mod overlay;

// Re-export from core
pub use core::{ChatView, ChatViewComponent, HistoryMessage, MouseAction, Selection, ToolStatus};

// Re-export from overlay
pub use overlay::{
    line_display_width, line_to_text, logical_to_visual_line, scan_code_blocks, CodeBlockOverlay,
    CodeBlockOverlayManager,
};
