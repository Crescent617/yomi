// Re-exported from TUI — keep in sync with crates/tui/src/components/input/mod.rs
export const SLASH_COMMANDS: readonly [readonly [string, string]] = [
  ["/new", "Create new session"],
  ["/goal", "<description> Start goal mode with optional description"],
  ["/goal:stop", "Stop goal mode"],
  ["/todos", "Toggle todo list visibility"],
  ["/yolo", "Toggle YOLO mode (auto-approve all tools)"],
  ["/browse", "Toggle browse mode"],
  ["/sessions", "Switch to another session"],
  ["/rewind", "Restore conversation/file checkpoint"],
  ["/undo", "Undo last turn"],
  ["/compact", "Force message compaction"],
  ["/reload", "Reload skills and hooks from disk"],
  ["/help", "Show keyboard shortcuts help"],
] as const;
