//! Utility functions for the kernel crate

pub mod asset;
pub mod attachments;
pub mod env;
pub mod file_chunk;
pub mod g_lock;
pub mod html;
pub mod http;
pub mod id;
pub mod image;
pub mod line_numbers;
pub mod logging;
pub mod path;
pub mod rg_helper;
pub mod search;
pub mod signal;
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
