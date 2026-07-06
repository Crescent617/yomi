//! Environment variable utilities for the kernel crate

/// Get environment variable - inlined for performance
#[inline]
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Try multiple env vars in order, return first set value
#[inline]
pub fn env_first(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| env_var(name))
}

/// Parse environment variable as a specific type
#[inline]
pub fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    env_var(name).and_then(|s| s.parse().ok())
}

/// Parse boolean from environment variable
#[inline]
pub fn env_bool(name: &str) -> bool {
    std::env::var(name).is_ok_and(|s| {
        matches!(
            s.as_bytes(),
            b"true" | b"1" | b"yes" | b"TRUE" | b"YES" | b"on"
        )
    })
}

/// Parse optional boolean from environment variable
#[inline]
pub fn env_bool_opt(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|s| {
        matches!(
            s.as_bytes(),
            b"true" | b"1" | b"yes" | b"TRUE" | b"YES" | b"on"
        )
    })
}

/// Parse number with unit suffix (k/m) from string
/// Supports formats like "131072", "128k", "200k", "1m"
pub fn parse_number_with_unit(s: &str) -> Option<u32> {
    let s = s.trim().to_lowercase();

    // Check for 'k' suffix (thousands)
    if let Some(num_str) = s.strip_suffix('k') {
        let num: f32 = num_str.parse().ok()?;
        return Some((num * 1000.0) as u32);
    }

    // Check for 'm' suffix (millions)
    if let Some(num_str) = s.strip_suffix('m') {
        let num: f32 = num_str.parse().ok()?;
        return Some((num * 1_000_000.0) as u32);
    }

    // Plain number
    s.parse().ok()
}

#[cfg(test)]
#[path = "env_test.rs"]
mod tests;
