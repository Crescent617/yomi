# TUI Redesign: Claude Code Style

## Design Goals

- **Minimalist**: Clean, distraction-free interface like Claude Code
- **Content-first**: No unnecessary borders or decorations
- **Efficient keyboard shortcuts**: Emacs-style input editing
- **Smart folding**: Tool outputs and thinking collapsed by default

## Visual Design

### Layout

```
> user's message here
  can be multiple lines
  with proper indentation

Assistant response starts here without prefix.
Normal markdown formatting **bold**, `code`.

```rust
fn main() {
    println!("code blocks have syntax highlighting");
}
```

▶ Tools (3)  [press Enter/Tab to expand]
▶ Thinking (256 tokens)

> user input at bottom with cursor
```

### Color Scheme

| Element | Color | Style |
|---------|-------|-------|
| User prefix `>` | Green (`#4CAF50`) | Bold |
| User text | White | Normal |
| Assistant text | White | Normal |
| Tool call border | Dark Gray (`#555`) | Box drawing chars |
| Tool name | Blue (`#64B5F6`) | Bold |
| Thinking text | Gray (`#888`) | Italic |
| Code blocks | Yellow (`#FFEB3B`) bg | Dark text |
| System messages | Dark Gray (`#666`) | Italic |
| Input cursor | Green block | Blinking |

### Role Display

- **User**: `>` prefix + green color
- **Assistant**: No prefix, normal white text
- **Tool**: Collapsible section with box-drawing border when expanded
- **Thinking**: Collapsible, gray italic when expanded
- **System**: Gray italic, minimal (only for important messages)

## Input Shortcuts

| Shortcut | Action | Notes |
|----------|--------|-------|
| `Enter` | Send message | When input is not empty |
| `Ctrl+J` | Newline | Insert literal newline |
| `Ctrl+C` (single) | Cancel current | Send cancel signal to agent |
| `Ctrl+C` (double, <1s) | Exit | Quit the application |
| `Ctrl+W` | Delete word | Backward-kill-word (Emacs style) |
| `Ctrl+U` | Delete line | Kill from cursor to start |
| `Ctrl+K` | Delete to end | Kill from cursor to end |
| `Ctrl+A` | Line start | Beginning of line |
| `Ctrl+E` | Line end | End of line |
| `Ctrl+L` | Clear screen | Redraw, keep history |
| `Ctrl+P` / `↑` | Prev history | Previous user message |
| `Ctrl+N` / `↓` | Next history | Next user message |
| `Tab` | Toggle fold | Expand/collapse Tool/Thinking sections |

## Message Components

### Tool Call Display

**Collapsed (default):**
```
▶ Tools (2)  read: main.rs, bash: cargo test
```

**Expanded:**
```
▼ Tools (2)
  ┌─ read: main.rs ─────────────────────────────────────────┐
  │ 1  use std::io;                                          │
  │ 2                                                          │
  │ 3  fn main() {                                            │
  │ ...                                                       │
  └───────────────────────────────────────────────────────────┘
  ┌─ bash: cargo test ──────────────────────────────────────┐
  │ running 5 tests                                           │
  │ test tests::test_ok ... ok                                │
  └───────────────────────────────────────────────────────────┘
```

### Thinking Display

**Collapsed (default):**
```
▶ Thinking (320 tokens)
```

**Expanded:**
```
▼ Thinking (320 tokens)
  I need to analyze this problem step by step.
  First, let me understand the requirements...
```

## State Management

### Folding State

Each collapsible section (Tool, Thinking) stores:
- `is_expanded: bool` - Current fold state
- `is_focused: bool` - For Tab navigation

### Input State

- `input_lines: Vec<String>` - Multi-line input buffer
- `cursor_line: usize` - Current line
- `cursor_col: usize` - Column position
- `history: Vec<String>` - Input history
- `history_index: Option<usize>` - Current history position

### Cancel State

- `last_ctrl_c: Option<Instant>` - Timestamp of last Ctrl+C
- `ctrl_c_threshold: Duration = 1s` - Double-press threshold

## Rendering Strategy

### Screen Layout

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  [Message history scrolls here]                             │
│                                                             │
│  > user message                                             │
│                                                             │
│  Assistant response with markdown                           │
│  ```code```                                                 │
│                                                             │
│  ▶ Tools (1)                                                │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│  > user input cursor here                                   │
└─────────────────────────────────────────────────────────────┘
```

### Scrolling Behavior

- Auto-scroll to bottom on new assistant content
- User scroll up → pause auto-scroll
- User press `End` or send message → resume auto-scroll
- Scroll offset tracked separately for message area

## Implementation Notes

### Ratatui Components

- `Paragraph` for message content
- `List` or custom for foldable sections
- Stateful list for fold navigation
- Custom widget for tool output boxes

### Event Handling

- Async event loop with `tokio::select!`
- Key events processed immediately
- Terminal resize handled gracefully
- Mouse events optional (for clicking folds)

### Performance

- Only redraw changed regions (when possible)
- Virtual scrolling for long histories (>1000 lines)
- Markdown parsed incrementally during streaming
