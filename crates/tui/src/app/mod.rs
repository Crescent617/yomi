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
