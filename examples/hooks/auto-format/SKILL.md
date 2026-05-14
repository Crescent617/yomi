---
description: Auto-format code after file edits
triggers:
  - format
hooks:
  - event: PostToolUse
    matcher: Edit|Write
    type: command
    command: "cargo fmt"
    timeout: 10
---

# Auto Format Skill

Runs `cargo fmt` automatically after any file edit or write.
Compatible with Codex / Claude Code hook conventions.
