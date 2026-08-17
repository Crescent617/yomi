//! Markdown-aware text helpers for the platform-neutral agent contracts
//! (attachments, mentions): content inside a fenced code block is an
//! example shown to the user, never a declaration.

/// Apply `f` to each contiguous run of `text` standing outside a fenced
/// code block; fenced runs (fence markers included) pass through verbatim.
/// A fence marker is a line whose first non-whitespace characters are
/// ``` ``` ``` (inline backticks never count). Runs handed to `f` may span
/// multiple lines, so multi-line blocks inside one run stay parseable.
pub fn map_outside_fences(text: &str, mut f: impl FnMut(&str, &mut String)) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    let mut run_start = 0usize;
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        let line_end = pos + line.len();
        if line.trim_start().starts_with("```") {
            if !fenced {
                f(&text[run_start..pos], &mut out);
            }
            fenced = !fenced;
            out.push_str(line);
            pos = line_end;
            if !fenced {
                run_start = pos;
            }
        } else {
            pos = line_end;
            if fenced {
                out.push_str(line);
            }
        }
    }
    if !fenced {
        f(&text[run_start..pos], &mut out);
    }
    out
}

#[cfg(test)]
#[path = "markdown_test.rs"]
mod tests;
