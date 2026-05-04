//! Utility functions for the kernel crate

pub mod env;
pub mod html;
pub mod id;
pub mod image;
pub mod line_numbers;
pub mod path;
pub mod strs;
pub mod tokens;

/// Get current unix timestamp in seconds
#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
