# Code Review: `gui` branch

> Generated: 2026-06-01  
> Scope: `kernel`, `tui`, `cli` crates (gui crate excluded — missing offline deps)

---

## ⚠️ Critical Bugs

| File | Line | Issue | Fix |
|------|------|-------|-----|
| `crates/kernel/src/checkpoint/store.rs` | 233, 243, 405 | `self.base_dir.parent().unwrap()` panics if `base_dir` is root `/` | Use `ok_or()` with proper error handling |
| `crates/tui/src/theme.rs` | 129 | `THEME_CONFIG.read().map(|t| *t).unwrap()` panics if RwLock is poisoned | Use `unwrap_or_default()` or handle gracefully |
| `crates/tui/src/app/model.rs` | 148 | `questions.first().cloned().unwrap()` panics if questions is empty | Use `if let Some(next) = questions.first()` |
| `crates/tui/src/components/chat_view/message_renderer.rs` | 707 | `s.chars().next().unwrap().is_uppercase()` panics on empty string | Add `!s.is_empty()` guard |
| `crates/tui/src/markdown_stream.rs` | 377 | `code_language.take().unwrap()` panics if None | Use `if let Some(lang)` or `unwrap_or_default()` |

---

## 🪦 Dead Code (`#[allow(dead_code)]`)

| File | Line | Item | Note |
|------|------|------|------|
| `crates/kernel/src/agent/message_buffer.rs` | 16 | Entire `impl` block | All methods marked dead — either remove or actually use |
| `crates/kernel/src/agent/agent.rs` | 1526 | `fn messages(&self)` | Marked dead but might be useful for debugging |
| `crates/kernel/src/tools/subagent.rs` | 479 | `do_record_token_usage()` | Stub with TODO — either implement or remove |
| `crates/kernel/src/skill.rs` | 22 | `SkillFrontmatter.name` | Kept for "backwards compatibility" but never read |
| `crates/kernel/src/utils/rg_helper.rs` | 84, 96, 121 | `RgMessage`, `BeginData`, `EndData` | Deserialization types that are matched but fields unused |
| `crates/kernel/src/storage/init.rs` | 19 | `pool` field | Kept for Clone but never accessed directly |
| `crates/gui/src/terminal/manager.rs` | 10 | (gui crate) | Gui crate has its own dead code |

---

## 🔁 Duplicated Code

| Pattern | Locations | Suggestion |
|---------|-----------|------------|
| **`resolve_skill_folders`** | `cli/src/commands/tui.rs:279`, `gui/src/daemon.rs:294` | Nearly identical — extract to `kernel::utils::path` |
| **`create_provider`** | `cli/src/commands/tui.rs:294`, `gui/src/daemon.rs:306` | Identical logic — extract to shared helper |
| **`truncate_by_chars`** | `kernel/src/utils/strs.rs:8`, `tui/src/utils/text.rs:65` | Two implementations with different suffix handling — unify or clearly differentiate |
| **`truncate_with_suffix`** | `kernel/src/utils/strs.rs:34`, `tui/src/utils/text.rs:116` | Byte-based vs width-based — names are confusingly similar |
| **Duplicate doc comment** | `cli/src/commands/tui.rs:277-278` | Same comment line repeated twice |

---

## 🐌 Performance / Improvement Opportunities

| File | Issue | Suggestion |
|------|-------|------------|
| `crates/kernel/src/utils/html.rs` | `normalize_whitespace` allocates `String` even when no changes needed | Return `Cow<str>` to avoid allocation |
| `crates/kernel/src/utils/html.rs` | `extract_text_from_element` pushes every text node as separate `String` | Use `&str` slices or a single buffer |
| `crates/kernel/src/utils/strs.rs` | `truncate_by_chars` iterates chars twice (`count()` then `enumerate()`) | Single pass with `take()` |
| `crates/kernel/src/client/mod.rs` | Heavy cloning of session IDs in event dispatch | Consider `Arc<str>` or `SmolStr` for SessionId |
| `crates/tui/src/table.rs` | `cell_lines.iter().map(|v| v.len()).max().unwrap_or(1)` duplicated | Extract helper |
| `crates/kernel/src/utils/tokens.rs` | `format_estimated_tokens` and `format_estimated_tokens_f64` are nearly identical | DRY — have one call the other |
| `crates/kernel/src/utils/tokens.rs` | `format_estimated_tokens` and `format_actual_tokens` share logic but one has `~` prefix | Extract shared formatter |

---

## 📝 TODOs / Unfinished

| File | Line | TODO |
|------|------|------|
| `crates/kernel/src/app/coordinator.rs` | 380 | `error: None` — "capture error from agent if needed" |
| `crates/kernel/src/checkpoint/store.rs` | 509 | "Need to restore from an earlier checkpoint" |
| `crates/kernel/src/tools/subagent.rs` | 486 | "Inject UsageStore to record subagent token usage" |
| `crates/gui/src/commands/skill.rs` | 9 | "Kernel wire protocol does not expose list_skills yet" |

---

## 🎨 Minor Issues

| File | Issue |
|------|-------|
| `crates/cli/src/commands/tui.rs` | Duplicate doc comment lines 277-278 |
| `crates/kernel/src/utils/line_numbers.rs` | `format_file_lines` is just an alias for `add_line_numbers` — remove or deprecate |
| `crates/kernel/src/utils/strs.rs` | `truncate_by_chars` doc says "ensures result never exceeds `max_chars` chars" but actually `max_chars + suffix.len()` |
| `crates/kernel/src/utils/env.rs` | All functions marked `#[inline]` — unnecessary micro-optimization, let compiler decide |

---

## ✅ Build Status

`cargo clippy -p kernel -p tui -p cli --offline` passes cleanly (no warnings with current lints). The `gui` crate cannot build offline due to missing `tauri-plugin-dialog` dependency.
