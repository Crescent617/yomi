---
description: Block dangerous shell commands and destructive git operations
triggers:
  - security
hooks:
  - event: PreToolUse
    matcher: Bash
    type: inline
    patterns:
      - "rm -rf"
      - "DROP TABLE"
      - "mkfs"
      - "dd if=/dev/zero"
    message: "⚠️ Dangerous command blocked by security skill"
  - event: PostToolUse
    matcher: Edit|Write
    type: inline
    append: "\n[Hook] File was modified — remember to run tests."
---

# Security Guard Skill

This skill automatically intercepts dangerous commands before they execute.
