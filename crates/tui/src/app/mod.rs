//! TUI Application
//!
//! Main application using tuirealm framework for component-based TUI.

pub mod types;

mod events;
mod init;
mod model;
mod run;
mod streaming;
mod update;
mod view;

pub use types::{format_short_id, AppMode, FeatureGates, OnInputHook, Model, TuiResult};
pub use run::run_tui;
