/// Truncate a string by byte length with a custom suffix (UTF-8 safe).
/// Finds a valid UTF-8 boundary before truncating.
pub fn truncate_with_suffix(s: &str, max_bytes: usize, suffix: &str) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    let target_len = max_bytes.saturating_sub(suffix.len());
    if target_len == 0 {
        return suffix.to_string();
    }

    let mut byte_idx = 0;

    for (char_count, (idx, ch)) in s.char_indices().enumerate() {
        if char_count >= target_len {
            byte_idx = idx;
            break;
        }
        byte_idx = idx + ch.len_utf8();
    }

    format!("{}{}", &s[..byte_idx], suffix)
}
