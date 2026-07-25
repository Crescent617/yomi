//! Unified chat view component
//!
//! Displays chat history + streaming message in a single scrollable view.

mod core;
mod message_renderer;
mod overlay;

// Re-export from message_renderer
pub use message_renderer::extract_text_from_blocks;

// Re-export from core
// Re-export from core
pub use core::{
    ChatView, ChatViewComponent, HistoryMessage, MouseAction, Selection, SubagentState,
    SubagentStatus, ToolStatus,
};

// Re-export from overlay
pub use overlay::{
    line_display_width, line_to_text, scan_code_blocks, CodeBlockOverlay, CodeBlockOverlayManager,
    ContextMenu, ContextMenuAction,
};
