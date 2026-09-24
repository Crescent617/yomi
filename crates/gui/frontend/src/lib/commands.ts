// Re-exported from TUI — keep in sync with crates/tui/src/components/input/mod.rs
export const SLASH_COMMANDS: readonly (readonly [string, string])[] = [
  ["/clear", "Clear session context (messages, file state, todos)"],
  ["/new", "Create new session"],
  ["/todos", "Toggle todo list visibility"],
  ["/yolo", "Toggle YOLO mode (auto-approve all tools)"],
  ["/browse", "Toggle browse mode"],
  ["/sessions", "Switch to another session"],
  ["/undo", "Undo last turn"],
  ["/compact", "Force message compaction"],
  ["/history", "Search and reuse a previous message"],
  ["/fork", "Fork current session into a new one with full context"],
  ["/continue", "Trigger agent to continue without new input"],
  ["/debug", "noti Emit 10 short-lived test notifications"],
  ["/help", "Show keyboard shortcuts help"],
  // GUI 先行；TUI 实现 /btw 后同步 TUI 列表（见头注）。
  ["/btw", "Ask a side question without touching session context (text only)"],
] as const;
