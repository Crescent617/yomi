//! Custom attribute constants for TUI components.
//!
//! This module centralizes all `Attribute::Custom` string literals to avoid
//! typos and make refactoring easier.

// =============================================================================
// Messages (ChatView)
// =============================================================================

/// Attribute for adding a user message.
pub const ADD_USER_MESSAGE: &str = "add_user_message";
/// Attribute for adding an assistant message.
pub const ADD_ASSISTANT_MSG: &str = "add_assistant_message";
/// Attribute for adding an error message.
pub const ADD_ERROR_MESSAGE: &str = "add_error_message";
/// Attribute for clearing chat history.
pub const CLEAR_HISTORY: &str = "clear_history";

// =============================================================================
// Streaming
// =============================================================================

/// Attribute for starting streaming state.
pub const START_STREAMING: &str = "start_streaming";
/// Attribute for stopping streaming state.
pub const STOP_STREAMING: &str = "stop_streaming";
/// Attribute for canceling streaming state.
pub const CANCEL_STREAMING: &str = "cancel_streaming";
/// Attribute for appending content.
pub const APPEND_CONTENT: &str = "append_content";
/// Attribute for appending thinking content.
pub const APPEND_THINKING: &str = "append_thinking";

// =============================================================================
// Queued Message
// =============================================================================

/// Attribute for setting queued message.
pub const SET_QUEUED_MESSAGE: &str = "set_queued_message";
/// Attribute for clearing queued message.
pub const CLEAR_QUEUED_MESSAGE: &str = "clear_queued_message";

// =============================================================================
// Scrolling
// =============================================================================

/// Attribute for scrolling up.
pub const SCROLL_UP: &str = "scroll_up";
/// Attribute for scrolling down.
pub const SCROLL_DOWN: &str = "scroll_down";
/// Attribute for page up.
pub const PAGE_UP: &str = "page_up";
/// Attribute for page down.
pub const PAGE_DOWN: &str = "page_down";
/// Attribute for scrolling to top.
pub const SCROLL_TO_TOP: &str = "scroll_to_top";
/// Attribute for scrolling to bottom.
pub const SCROLL_TO_BOTTOM: &str = "scroll_to_bottom";
/// Attribute for getting scroll progress.
pub const SCROLL_PROGRESS: &str = "scroll_progress";
/// Attribute for setting scroll progress.
pub const SET_SCROLL_PROGRESS: &str = "set_scroll_progress";
/// Attribute for clearing scroll progress.
pub const CLEAR_SCROLL_PROGRESS: &str = "clear_scroll_progress";

// =============================================================================
// Tools
// =============================================================================

/// Attribute for starting tool execution.
pub const START_TOOL: &str = "start_tool";
/// Attribute for completing tool execution.
pub const COMPLETE_TOOL: &str = "complete_tool";
/// Attribute for failing tool execution.
pub const FAIL_TOOL: &str = "fail_tool";
/// Attribute for updating tool progress.
pub const UPDATE_TOOL_PROGRESS: &str = "update_tool_progress";
/// Attribute for clearing the tool call display.
pub const CLEAR_TOOL_CALL: &str = "clear_tool_call";
/// Attribute for appending tool call delta.
pub const APPEND_TOOL_CALL_DELTA: &str = "append_tool_call_delta";

// =============================================================================
// Expand/Collapse
// =============================================================================

/// Attribute for toggling expand all.
pub const TOGGLE_EXPAND_ALL: &str = "toggle_expand_all";
/// Attribute for expanding all items.
pub const EXPAND_ALL: &str = "expand_all";
/// Attribute for collapsing all items.
pub const COLLAPSE_ALL: &str = "collapse_all";
/// Attribute for toggling thinking visibility.
pub const TOGGLE_THINKING: &str = "toggle_thinking";

// =============================================================================
// Banner & Info
// =============================================================================

/// Attribute for setting banner/skills info.
pub const SET_BANNER: &str = "set_banner";
/// Attribute for setting skills list.
pub const SKILLS: &str = "skills";
/// Attribute for initializing chat history.
pub const INIT_HISTORY: &str = "init_history";

// =============================================================================
// Status Bar
// =============================================================================

/// Attribute for setting the input mode.
pub const SET_MODE: &str = "set_mode";
/// Attribute for getting the current input mode.
pub const MODE: &str = "mode";
/// Attribute for setting permission level.
pub const SET_PERMISSION_LEVEL: &str = "set_permission_level";
/// Attribute for showing a tip message.
pub const SHOW_TIP: &str = "show_tip";
/// Attribute for clearing the tip message.
pub const CLEAR_TIP: &str = "clear_tip";
/// Attribute for setting context usage.
pub const SET_CTX_USAGE: &str = "set_ctx_usage";
/// Attribute for clearing the status message.
pub const CLEAR_MESSAGE: &str = "clear_message";

// =============================================================================
// Info Bar
// =============================================================================

/// Attribute for showing a notification.
pub const SHOW_NOTIFICATION: &str = "show_notification";
/// Attribute for clearing a notification.
pub const CLEAR_NOTIFICATION: &str = "clear_notification";
/// Attribute for starting compacting state.
pub const START_COMPACTING: &str = "start_compacting";
/// Attribute for stopping compacting state.
pub const STOP_COMPACTING: &str = "stop_compacting";

// =============================================================================
// Dialog / Picker
// =============================================================================

/// Attribute for showing a dialog/picker.
pub const DIALOG_SHOW: &str = "dialog_show";
/// Attribute for hiding a dialog/picker.
pub const DIALOG_HIDE: &str = "dialog_hide";
/// Attribute for setting items in fuzzy picker.
pub const PICKER_ITEMS: &str = "picker_items";
/// Attribute for setting query in fuzzy picker.
pub const PICKER_QUERY: &str = "picker_query";

// =============================================================================
// Todo List
// =============================================================================

/// Attribute for updating todo list content (JSON string).
pub const SET_TODOS: &str = "set_todos";
/// Attribute for clearing todo list.
pub const CLEAR_TODOS: &str = "clear_todos";
/// Attribute for toggling todo list visibility.
pub const TOGGLE_TODOS: &str = "toggle_todos";

// =============================================================================
// Input
// =============================================================================

/// Attribute for setting command history.
pub const HISTORY: &str = "history";
/// Attribute for setting the working directory display.
pub const WORKING_DIR: &str = "working_dir";
/// Attribute for setting input content.
pub const INPUT_CONTENT: &str = "input_content";

// =============================================================================
// Animation
// =============================================================================

/// Attribute for ticking/updating animation frame.
pub const TICK: &str = "tick";
