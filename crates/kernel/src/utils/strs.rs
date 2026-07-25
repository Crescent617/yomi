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

/// Unicode-safe tail slice: the last `max_chars` chars of `s`
/// (the whole string when it is shorter).
pub fn tail_by_chars(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    s.chars().skip(count - max_chars).collect()
}

/// The largest byte index <= `target` that lies on a UTF-8 char boundary.
pub fn floor_char_boundary(s: &str, target: usize) -> usize {
    if target >= s.len() {
        return s.len();
    }
    let mut idx = target;
    while !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
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
        // The suffix alone meets/exceeds the cap — hard-truncate it so the
        // result still never exceeds `max_bytes`.
        return truncate_with_suffix(suffix, max_bytes, "");
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

/// Truncate a string by UTF-16 code unit count with a custom suffix.
///
/// Some platforms (e.g. Telegram) measure text length in UTF-16 code units,
/// so a char-count cap still overflows on non-BMP characters (emoji, CJK
/// extensions). The result is always valid UTF-8: splitting happens at char
/// boundaries by construction.
///
/// # Behavior
/// - If UTF-16 unit count <= `max_units`: returns `s` as-is (no suffix added)
/// - Otherwise keeps as many whole chars as fit within the remaining unit
///   budget and appends `suffix`
pub fn truncate_by_utf16(s: &str, max_units: usize, suffix: &str) -> String {
    if s.encode_utf16().count() <= max_units {
        return s.to_string();
    }
    let budget = max_units.saturating_sub(suffix.encode_utf16().count());
    if budget == 0 {
        // The suffix alone meets/exceeds the cap — hard-truncate it so the
        // result still never exceeds `max_units`.
        return truncate_by_utf16(suffix, max_units, "");
    }
    let mut result = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let units = ch.len_utf16();
        if used + units > budget {
            break;
        }
        result.push(ch);
        used += units;
    }
    result.push_str(suffix);
    result
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
