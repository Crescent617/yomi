//! TUI Application
//!
//! Main application using tuirealm framework for component-based TUI.

pub mod types;

mod event_pump;
mod events;
mod init;
mod model;
mod run;
mod streaming;
mod update;
mod view;

pub use run::run_tui;
pub use types::{format_short_id, AppMode, FeatureGates, Model, TuiResult};

use kernel::types::SessionMessage;

/// Calculate total token usage from session messages.
/// Tries to use the last message's token usage first, otherwise estimates from all messages.
pub fn calc_token_usage(messages: &[SessionMessage]) -> u32 {
    messages
        .iter()
        .filter_map(|m| m.token_usage().map(|u| u.total_tokens))
        .next_back()
        .unwrap_or_else(|| {
            use kernel::utils::tokens;
            messages
                .iter()
                .map(|m| tokens::estimate_tokens(&m.text_content()))
                .sum::<usize>() as u32
        })
}
