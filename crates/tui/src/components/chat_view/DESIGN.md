# ChatView Rendering Architecture Redesign

> **Note:** This document captures the design intent of the refactor. The actual
> implementation has since evolved: `WrapCache` was renamed to `WrapInfo` and moved
> out of `chat_view/`; the per-frame rendering described in §4.4 (building a
> cropped `Vec<Line>` manually) was replaced by `WrapParagraph`, which zero-copy
> borrows `&[Arc<Line>]` + `&WrapInfo` and handles scroll/crop internally. Banner
> removal and cache simplification described here remain accurate.

## 1. Problem Diagnosis

### 1.1 Cache Layer Proliferation
Current architecture maintains 4+ layers of intermediate buffers:

```
HistoryMessage[]
    → msg_cache: Vec<Option<Vec<Arc<Line>>>>    (message-level, may be None)
    → msg_lines: Vec<Arc<Line>>                  (flattened history)
    → all_lines_buf: Vec<Arc<Line>>              (history + banner + streaming + queued)
    → Viewport::lines: Vec<ViewportLine>         (cropped visible subset)
    → WrapParagraph::Text                        (final render)
```

Each layer has its own dirty-tracking logic (`msg_cache_dirty`, `banner_dirty`, `suffix_changed`), creating a fragile chain of invalidation.

### 1.2 Banner Coupling
Banner (mascot + info) is embedded inside ChatView and scrolls with messages. This introduces:
- `banner_cache`, `banner_dirty`, `mascot_animator` fields unrelated to chat
- `banner_in_viewport()` visibility check on every tick
- Special-case prefix logic in wrap cache (`prefix_len` must account for banner lines)
- Animated component (mascot blink) mixed with static chat content

### 1.3 Viewport Over-Abstraction
`Viewport` struct duplicates wrap calculations:
- `Viewport::new_cropped()` calls `calc_wrap_boundaries()` to compute height
- `WrapParagraph` calls `calc_wrap_boundaries()` again to render the same line
- `Viewport::screen_to_position()` re-implements wrap row logic that `WrapParagraph` already has

The "cropping" functionality (showing a line from mid-wrap) should live in the renderer, not in a coordinate-mapping abstraction.

### 1.4 WrapCache `prefix_len` Ambiguity
`rebuild(&lines, width, prefix_len)` uses `prefix_len` to mean "lines before this index are guaranteed unchanged". But:
- When a historical message is invalidated (e.g., tool completes), only `msg_cache[i]` changes
- `rebuild_msg_cache()` truncates and rebuilds from `first_dirty`, but passes `msg_lines.len()` as `prefix_len`
- This is **incorrect** because `msg_lines` may have changed in length, but `prefix_len` claims they're stable
- The bug was masked by a `lines.len() == self.heights.len()` early-return that skipped rebuild when line count didn't change (now fixed)

### 1.5 Probability of "Last Lines Not Showing"
Streaming appends content to the same logical line (no new `\n`). `all_lines_buf` line count stays the same → `wrap_cache.rebuild()` skips → wrap heights stay stale (too small) → `total_visual_lines` underestimated → `visual_scroll` too small → viewport starts too high → last lines clipped off screen.

This only "randomly" worked when a token happened to trigger a new markdown element (list, code fence, etc.) that increased logical line count.

---

## 2. Target Architecture

### 2.1 Principle: Append-Only History, Per-Frame Suffix

History messages are append-only (new messages added to end). Individual messages may be **replaced** (toggle thinking, tool complete), but this is infrequent.

Streaming content is the **only** thing that changes every frame. It should be treated as a volatile suffix, not cached.

### 2.2 Simplified Cache Hierarchy

```
HistoryMessage[]
    → msg_cache: Vec<Vec<Arc<Line<'static>>>>    (1:1 with messages, always valid)
    → flat_lines: Vec<Arc<Line<'static>>>         (flattened msg_cache + separators)
    → all_lines: Vec<Arc<Line<'static>>>          (flat_lines + streaming + queued)
    → WrapCache                                   (wrap heights for all_lines)
    → WrapParagraph                               (render with scroll_y + selection)
```

**Eliminated**: `msg_cache` Options, `banner_cache`, `Viewport` struct, `msg_lines` as separate persistent buffer, `all_lines_buf` as persistent field.

### 2.3 Banner Removal

Banner is **removed from ChatView entirely**. ChatView renders only chat messages (history + streaming + queued). No banner scrolling, no mascot animation inside chat.

If a welcome banner is desired in the future, it should be a separate component above ChatView in the layout, not inside the scrollable message area.

```
Layout::Vertical[
    ChatView,    // pure messages, scrollable
    InfoBar,
    InputBox,
    StatusBar,
]
```

---

## 3. Data Structures

### 3.1 ChatView Fields (After)

```rust
pub struct ChatView {
    // === Core state ===
    props: Props,
    messages: Vec<HistoryMessage>,

    // === Caches: two layers ===
    /// Per-message rendered lines. Replaced on invalidate, pushed on new msg.
    msg_cache: Vec<Vec<Arc<Line<'static>>>>,
    /// Flattened msg_cache + separators. Rebuilt when msg_cache_dirty.
    flat_lines: Vec<Arc<Line<'static>>>,
    /// flat_lines + streaming + queued. Rebuilt every frame (clear+extend).
    all_lines: Vec<Arc<Line<'static>>>,

    // === Streaming (volatile, not cached) ===
    streaming_content: String,
    streaming_thinking: String,
    is_streaming: bool,
    md_renderer: StreamingMarkdownRenderer,
    queued_message: Option<Vec<ContentBlock>>,

    // === Scroll ===
    scroll_offset: usize,          // visual rows from bottom
    last_visible_height: usize,
    wrap_cache: WrapCache,

    // === Tools ===
    active_tools: HashMap<String, (String, ToolStatus)>,
    expand_all: bool,

    // === Selection & mouse ===
    selection: Option<Selection>,
    is_selecting: bool,
    last_click_time: Option<Instant>,
    last_click_pos: Option<(usize, usize)>,
    current_area: Option<Rect>,
    scroll_button_area: Option<Rect>,

    // === Overlays ===
    code_block_overlay_manager: CodeBlockOverlayManager,
    context_menu: Option<ContextMenu>,
}
```

**Removed fields**: `banner`, `banner_cache`, `banner_dirty`, `mascot_animator`, `msg_lines`, `all_lines_buf`, `total_visual_lines`, `viewport`.

### 3.2 Dirty Flags

| Flag | Meaning | Cleared When |
|------|---------|--------------|
| `msg_cache_dirty` | `msg_cache[i]` was replaced or new msg pushed | `rebuild_flat_lines()` in `view()` |
| (implicit) | streaming content changed | every frame rebuilds suffix |

No other dirty flags. `all_lines` is rebuilt unconditionally on every `view()` call.

---

## 4. Data Flow

### 4.1 Adding a Message (User / Assistant / Tool Start)

```
add_user_message(blocks)
    → messages.push(User(blocks))
    → msg_cache.push(render_message(...))      // immediate render
    → msg_cache_dirty = true
```

### 4.2 Invalidating a Message (Tool Complete / Toggle Thinking)

```
complete_tool(tool_id, output, ...)
    → find message[i] in messages, update fields
    → msg_cache[i] = render_message(&messages[i], width)
    → msg_cache_dirty = true
```

No `Option::None` indirection. Direct replacement.

### 4.3 Streaming Append

```
append_streaming_content(text)
    → streaming_content.push_str(text)
    → md_renderer.append(text)
    // No cache dirty flags. Streaming is suffix, rebuilt every frame.
```

### 4.4 View Frame

```rust
fn view(&mut self, frame: &mut Frame, area: Rect) {
    let width = area.width as usize;
    let height = area.height as usize;
    self.last_visible_height = height;
    self.current_area = Some(area);

    // 1. Check if historical messages changed BEFORE clearing the flag
    let msg_changed = self.msg_cache_dirty;

    // 2. Rebuild flat_lines if any historical message changed
    if msg_changed {
        self.rebuild_flat_lines();
        self.msg_cache_dirty = false;
    }

    // 3. Rebuild all_lines unconditionally (clear+extend, cheap Arc clones)
    self.all_lines.clear();
    self.all_lines.extend(self.flat_lines.iter().cloned());
    if self.is_streaming || !self.streaming_content.is_empty() {
        self.all_lines.extend(self.render_streaming());
    }
    if let Some(ref queued) = self.queued_message {
        self.all_lines.extend(Self::render_queued_message(queued));
    }

    // 4. Rebuild wrap cache
    //    - msg changed: full rebuild (prefix_len = 0)
    //    - only streaming changed: reuse flat_lines as prefix
    let prefix_len = if msg_changed { 0 } else { self.flat_lines.len() };
    self.wrap_cache.rebuild(&self.all_lines, width, prefix_len);

    // 5. Scroll calculation
    let total_visual = self.wrap_cache.total_lines();
    let max_scroll = total_visual.saturating_sub(height);
    self.scroll_offset = self.scroll_offset.min(max_scroll);
    let visual_scroll = total_visual
        .saturating_sub(height)
        .saturating_sub(self.scroll_offset);

    // 6. Viewport: find start line and crop offset
    let (start_idx, crop_row) = self.wrap_cache.viewport_start(visual_scroll);

    // 7. Build visible text with first-line cropping
    let mut visible_lines: Vec<Line<'static>> = Vec::with_capacity(height);
    let mut rows_filled = 0;
    for (i, line) in self.all_lines.iter().enumerate().skip(start_idx) {
        if rows_filled >= height { break; }

        let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let boundaries = calc_wrap_boundaries(&line_text, width);
        let line_height = boundaries.len().max(1);

        let crop_start = if i == start_idx && crop_row > 0 {
            boundaries.get(crop_row).copied().unwrap_or(line_text.len())
        } else {
            0
        };

        let cropped = extract_line_segment(line, crop_start, line_text.len());
        visible_lines.push(cropped);
        rows_filled += if i == start_idx && crop_row > 0 {
            line_height - crop_row
        } else {
            line_height
        };
    }

    // 8. Selection: global → local viewport coordinates
    let local_sel = self.selection.and_then(|sel| {
        global_selection_to_viewport(sel, start_idx, crop_row, &self.all_lines, width)
    });

    // 9. Render
    let paragraph = WrapParagraph::new(Text::from(visible_lines))
        .selection(local_sel)
        .highlight_style(highlight_style);
    frame.render_widget(paragraph, area);

    // 10. Overlays (code blocks, scroll button, context menu)
    self.render_code_block_buttons(frame, area, visual_scroll);
    if self.is_scrolled_up() { self.draw_scroll_button(frame, area); }
    if let Some(ref menu) = self.context_menu { menu.render(frame); }
}
```

### 4.5 WrapCache Rebuild Rules

```rust
fn rebuild(&mut self, lines: &[Arc<Line>], width: usize, prefix_len: usize) {
    let safe_prefix = prefix_len.min(self.heights.len()).min(lines.len());

    // Force full rebuild on resize or shrink
    if width != self.width || lines.len() < self.heights.len() || safe_prefix == 0 {
        self.clear();
        self.width = width;
        for line in lines { self.push_line(line, width); }
        return;
    }

    // Skip only if stable prefix covers entire buffer
    if safe_prefix >= lines.len() {
        return;
    }

    // Reuse prefix, rebuild suffix
    self.heights.truncate(safe_prefix);
    self.prefix.truncate(safe_prefix);
    for line in lines.iter().skip(safe_prefix) {
        self.push_line(line, width);
    }
}
```

---

## 5. Coordinate Conversion (Replacing Viewport)

### 5.1 Screen → Global Position (Mouse Click)

```rust
fn screen_to_position(&self, mouse_x: u16, mouse_y: u16) -> Option<(usize, usize)> {
    let area = self.current_area?;
    if !area.contains(mouse_x, mouse_y) { return None; }

    let terminal_row = (mouse_y - area.y) as usize;
    let terminal_col = (mouse_x - area.x) as usize;
    let width = area.width as usize;

    // Use wrap_cache to find which logical line / wrap row this screen row maps to
    let visual_scroll = self.wrap_cache.total_lines()
        .saturating_sub(self.last_visible_height)
        .saturating_sub(self.scroll_offset);
    let target_visual_row = visual_scroll + terminal_row;

    let (logical_line, row_in_line) = self.wrap_cache.visual_to_logical(target_visual_row);
    let line = self.all_lines.get(logical_line)?;
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let boundaries = calc_wrap_boundaries(&text, width);

    let start_byte = boundaries.get(row_in_line).copied().unwrap_or(0);
    let end_byte = boundaries.get(row_in_line + 1).copied().unwrap_or(text.len());
    let char_col = display_col_to_char_idx(&text, start_byte, end_byte, terminal_col);

    Some((logical_line, char_col))
}
```

### 5.2 Selection Global → Local

```rust
fn global_selection_to_viewport(
    sel: Selection,
    start_idx: usize,
    crop_row: usize,
    all_lines: &[Arc<Line>],
    width: usize,
) -> Option<SelectionRange> {
    let norm = sel.normalized();
    let viewport_end = all_lines.len(); // approximate upper bound

    if norm.end_line < start_idx || norm.start_line >= viewport_end {
        return None;
    }

    let local_start_line = norm.start_line.saturating_sub(start_idx);
    let local_end_line = (norm.end_line - start_idx).min(all_lines.len() - 1);

    // Adjust start column for first line crop
    let adjusted_start_col = if norm.start_line == start_idx && crop_row > 0 {
        let line = &all_lines[start_idx];
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let boundaries = calc_wrap_boundaries(&text, width);
        let crop_byte = boundaries.get(crop_row).copied().unwrap_or(0);
        let crop_chars = text[..crop_byte].chars().count();
        norm.start_col.saturating_sub(crop_chars)
    } else {
        norm.start_col
    };

    Some((
        (local_start_line, adjusted_start_col),
        (local_end_line, if norm.end_line >= viewport_end { usize::MAX } else { norm.end_col }),
    ))
}
```

---

## 6. File Changes

### 6.1 Deleted Files
- `crates/tui/src/components/chat_view/viewport.rs` — over-abstracted, logic inlined

### 6.2 Modified Files

| File | Change |
|------|--------|
| `core.rs` | Remove banner fields/methods. Simplify cache to `msg_cache: Vec<Vec<...>>`. Inline viewport logic in `view()`. Remove `total_visual_lines` field. |
| `wrap_cache.rs` | Keep, fix `safe_prefix >= lines.len()` skip condition (already done). |
| `mod.rs` | Remove `pub use viewport::*`. Remove `mod viewport`. |
| `overlay.rs` | No changes needed (code block overlays use logical line indices, still valid). |
| `app/view.rs` | ChatView chunk no longer includes banner area. |
| `app/init.rs` | Remove `update_banner()`. Remove `SET_BANNER` attr setup. |

### 6.3 Deleted Code (Approximate)
- `Viewport` struct and `ViewportLine` (~200 lines)
- `banner_cache`, `banner_dirty`, `mascot_animator` fields
- `set_banner()`, `rebuild_banner_cache()`, `banner_in_viewport()`
- Tick mascot handling
- `msg_lines` persistent buffer (replaced by `flat_lines`)
- `all_lines_buf` persistent field (replaced by `all_lines`)
- `total_visual_lines` field (use `wrap_cache.total_lines()`)
- `viewport: Option<Viewport>` field

---

## 7. Performance

### 7.1 Frame Budget (60 FPS = 16.6ms)

| Operation | Cost | Notes |
|-----------|------|-------|
| `rebuild_flat_lines()` | ~O(n) Arc clones | Only when `msg_cache_dirty`. With 1000 lines: ~20μs |
| `all_lines.clear() + extend()` | ~O(n) Arc clones | Every frame. With 2000 lines: ~40μs |
| `wrap_cache.rebuild(suffix)` | ~O(suffix) wrap calc | Streaming only. Suffix typically <50 lines |
| `wrap_cache.viewport_start()` | O(log n) binary search | Negligible |
| Build visible Lines | O(visible) crop + clone | ~height lines, each clone is small |
| WrapParagraph render | O(visible × width) | Already done today |

**Total per-frame overhead of new architecture: <100μs** (< 1% of frame budget).

### 7.2 Memory

`all_lines` retains capacity between frames (no reallocation). `flat_lines` similarly retains capacity. Peak memory unchanged.

---

## 8. Correctness Invariants

1. `msg_cache.len() == messages.len()` always.
2. Every element of `msg_cache` is a valid `Vec<Arc<Line>>` (no `None`).
3. `flat_lines` is a faithful flattening of `msg_cache` + separators when `!msg_cache_dirty`.
4. `wrap_cache` is valid for `all_lines` after `view()` returns.
5. `scroll_offset` is clamped to `[0, total_visual - height]`.
6. `visual_scroll = total_visual - height - scroll_offset` is the global visual row at the top of the viewport.
7. First visible line may be cropped at `crop_row`; all subsequent lines are full.

---

## 9. Migration Steps

### Step 1: Banner Removal
- Remove `banner`, `banner_cache`, `banner_dirty`, `mascot_animator` fields from ChatView
- Delete `set_banner()`, `rebuild_banner_cache()`, `banner_in_viewport()` methods
- Remove mascot tick handling from `tick()`
- Remove banner lines from `view()` (no more `banner_changed`, no banner prefix in `all_lines`)
- Remove `SET_BANNER` attr handler and banner `query()` response
- Delete `update_banner()` from `app/init.rs`

### Step 2: Cache Simplification
- Change `msg_cache` type to `Vec<Vec<Arc<Line>>>`
- Update `invalidate_msg_cache()` to direct replacement
- Update `push_new_msg_cache()` to immediate render
- Introduce `flat_lines` and `all_lines` fields
- Remove `msg_lines`, `all_lines_buf`, `total_visual_lines`

### Step 3: Viewport Inline
- Delete `viewport.rs`
- Inline `viewport_start` + `crop_line` logic into `view()`
- Inline coordinate conversion into `screen_to_position()`
- Inline selection conversion into helper

### Step 4: Cleanup
- Remove dead imports
- Update tests
- Verify streaming scroll-to-bottom correctness
