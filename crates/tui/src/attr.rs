//! Custom attribute constants for TUI components.
//!
//! This module centralizes all `Attribute::Custom` string literals to avoid
//! typos and make refactoring easier.

/// Attribute for setting the input mode.
pub const SET_MODE: &str = "set_mode";
/// Attribute for getting the current input mode.
pub const MODE: &str = "mode";
/// Attribute for setting command history.
pub const HISTORY: &str = "history";
/// Attribute for setting the working directory display.
pub const WORKING_DIR: &str = "working_dir";
/// Attribute for initializing chat history.
pub const INIT_HISTORY: &str = "init_history";
/// Attribute for setting permission level.
pub const SET_PERMISSION_LEVEL: &str = "set_permission_level";
/// Attribute for showing a tip message.
pub const SHOW_TIP: &str = "show_tip";
/// Attribute for clearing the tip message.
pub const CLEAR_TIP: &str = "clear_tip";
/// Attribute for setting context usage.
pub const SET_CTX_USAGE: &str = "set_ctx_usage";
/// Attribute for setting banner/skills info.
pub const SET_BANNER: &str = "set_banner";
/// Attribute for setting skills list.
pub const SKILLS: &str = "skills";
/// Attribute for getting scroll progress.
pub const SCROLL_PROGRESS: &str = "scroll_progress";
/// Attribute for setting scroll progress.
pub const SET_SCROLL_PROGRESS: &str = "set_scroll_progress";
/// Attribute for clearing scroll progress.
pub const CLEAR_SCROLL_PROGRESS: &str = "clear_scroll_progress";
/// Attribute for adding an assistant message with thinking.
pub const ADD_ASSISTANT_WITH_THINKING: &str = "add_assistant_with_thinking";
/// Attribute for starting streaming state.
pub const START_STREAMING: &str = "start_streaming";
/// Attribute for stopping streaming state.
pub const STOP_STREAMING: &str = "stop_streaming";
/// Attribute for canceling streaming state.
pub const CANCEL_STREAMING: &str = "cancel_streaming";
/// Attribute for clearing the tool call display.
pub const CLEAR_TOOL_CALL: &str = "clear_tool_call";
/// Attribute for clearing the status message.
pub const CLEAR_MESSAGE: &str = "clear_message";
/// Attribute for scrolling to bottom.
pub const SCROLL_TO_BOTTOM: &str = "scroll_to_bottom";
/// Attribute for scrolling to top.
pub const SCROLL_TO_TOP: &str = "scroll_to_top";
/// Attribute for scrolling up.
pub const SCROLL_UP: &str = "scroll_up";
/// Attribute for scrolling down.
pub const SCROLL_DOWN: &str = "scroll_down";
/// Attribute for page up.
pub const PAGE_UP: &str = "page_up";
/// Attribute for page down.
pub const PAGE_DOWN: &str = "page_down";
/// Attribute for adding an error message.
pub const ADD_ERROR_MESSAGE: &str = "add_error_message";
/// Attribute for appending thinking content.
pub const APPEND_THINKING: &str = "append_thinking";
/// Attribute for appending content.
pub const APPEND_CONTENT: &str = "append_content";
/// Attribute for appending tool call delta.
pub const APPEND_TOOL_CALL_DELTA: &str = "append_tool_call_delta";
/// Attribute for showing a notification.
pub const SHOW_NOTIFICATION: &str = "show_notification";
/// Attribute for clearing a notification.
pub const CLEAR_NOTIFICATION: &str = "clear_notification";
/// Attribute for starting compacting state.
pub const START_COMPACTING: &str = "start_compacting";
/// Attribute for stopping compacting state.
pub const STOP_COMPACTING: &str = "stop_compacting";
/// Attribute for starting tool execution.
pub const START_TOOL: &str = "start_tool";
/// Attribute for completing tool execution.
pub const COMPLETE_TOOL: &str = "complete_tool";
/// Attribute for failing tool execution.
pub const FAIL_TOOL: &str = "fail_tool";
/// Attribute for updating tool progress.
pub const UPDATE_TOOL_PROGRESS: &str = "update_tool_progress";
/// Attribute for showing a component/dialog.
pub const SHOW: &str = "show";
/// Attribute for hiding a component/dialog.
pub const HIDE: &str = "hide";
/// Attribute for adding a user message.
pub const ADD_USER_MESSAGE: &str = "add_user_message";
/// Attribute for collapsing all items.
pub const COLLAPSE_ALL: &str = "collapse_all";
/// Attribute for toggling expand all.
pub const TOGGLE_EXPAND_ALL: &str = "toggle_expand_all";
/// Attribute for clearing history.
pub const CLEAR_HISTORY: &str = "clear_history";
/// Attribute for setting items (for fuzzy picker).
pub const ITEMS: &str = "items";
/// Attribute for setting query (for fuzzy picker).
pub const QUERY: &str = "query";
/// Attribute for setting content.
pub const SET_CONTENT: &str = "set_content";
/// Attribute for ticking/updating animation frame.
pub const TICK: &str = "tick";
