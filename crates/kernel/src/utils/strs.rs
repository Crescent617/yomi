/// Truncate a string by character count with a custom suffix.
///
/// # Behavior
/// - If char count <= `max_chars`: returns `s` as-is (no suffix added)
/// - If char count > `max_chars`: truncates to `max_chars` chars and appends suffix
///
/// This ensures the result never exceeds `max_chars` characters (plus suffix).
pub fn truncate_by_chars(s: &str, max_chars: usize, suffix: &str) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }

    let mut result = String::with_capacity(max_chars + suffix.len());
    for (i, ch) in s.chars().enumerate() {
        if i >= max_chars {
            break;
        }
        result.push(ch);
    }
    result.push_str(suffix);
    result
}

/// Truncate a string by byte length with a custom suffix (UTF-8 safe).
/// Finds a valid UTF-8 boundary before truncating.
///
/// # Behavior
/// - If `s.len() <= max_bytes`: returns `s` as-is (no suffix added)
/// - If `s.len() > max_bytes`: truncates to `max_bytes - suffix.len()` bytes
///   and appends `suffix`
///
/// This ensures the result never exceeds `max_bytes` bytes.
pub fn truncate_with_suffix(s: &str, max_bytes: usize, suffix: &str) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    let target_len = max_bytes.saturating_sub(suffix.len());
    if target_len == 0 {
        return suffix.to_string();
    }

    let mut byte_idx = 0;

    for (idx, ch) in s.char_indices() {
        // Check if adding this character would exceed target length
        if idx + ch.len_utf8() > target_len {
            break;
        }
        byte_idx = idx + ch.len_utf8();
    }

    format!("{}{}", &s[..byte_idx], suffix)
}

/// Truncate a string by keeping head and tail, omitting the middle.
///
/// # Behavior
/// - If `s.len() <= max_bytes`: returns `s` as-is (no allocation)
/// - If `s.len() > max_bytes`: keeps the first ~`max_bytes/2` bytes and the
///   last ~`max_bytes/2` bytes, joined by `sep`
///
/// This is UTF-8 safe: it never splits a multi-byte character.
pub fn truncate_keep_edges(s: &str, max_bytes: usize, sep: &str) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    if max_bytes <= sep.len() {
        // Can't fit both content and separator, return truncated separator
        return truncate_with_suffix(sep, max_bytes, "");
    }

    let content_budget = max_bytes.saturating_sub(sep.len());
    let head_budget = content_budget / 2;
    let tail_budget = content_budget - head_budget;

    // Find head boundary (valid UTF-8)
    let mut head = 0;
    for (i, c) in s.char_indices() {
        if i + c.len_utf8() > head_budget {
            break;
        }
        head = i + c.len_utf8();
    }

    // Find tail start boundary (valid UTF-8)
    // Scan from the end backwards, expanding the tail window as long as it fits
    let mut tail_start = s.len();
    for (i, _) in s.char_indices().rev() {
        if s.len() - i <= tail_budget {
            tail_start = i;
        } else {
            break;
        }
    }

    format!("{}{}{}", &s[..head], sep, &s[tail_start..])
}

#[macro_export]
macro_rules! const_concat {
    ($a:expr $(,)?) => {
        $a
    };

    ($($args:expr),+ $(,)?) => {{
        // 1️⃣ 编译期计算总长度
        const LEN: usize = 0 $(+ $args.len())+;

        // 2️⃣ 构造 buffer
        const BYTES: [u8; LEN] = {
            let mut out = [0u8; LEN];
            let mut offset = 0;

            $(
                {
                    let (new_out, new_offset) = $crate::utils::strs::push_str(out, offset, $args);
                    out = new_out;
                    offset = new_offset;
                }
            )+

            // Silence unused_assignments warning for the final offset update
            let _ = offset;

            out
        };
        unsafe { std::str::from_utf8_unchecked(&BYTES) }
    }};
}

pub const fn push_str<const N: usize>(
    mut out: [u8; N],
    offset: usize,
    s: &str,
) -> ([u8; N], usize) {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut off = offset;

    while i < bytes.len() {
        out[off] = bytes[i];
        off += 1;
        i += 1;
    }

    (out, off)
}

#[cfg(test)]
#[path = "strs_test.rs"]
mod tests;
