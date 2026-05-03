//! Session state storage - append-only JSONL file for per-session data.
//!
//! File format (`data_dir/sessions/{session_id}.state.jsonl`):
//! ```jsonl
//! {"t":"meta","v":1,"created":"2026-05-03T10:00:00Z"}
//! {"t":"file","p":"/home/user/src/main.rs","m":1714723200}
//! ```

mod entry;
mod manager;

pub use entry::{FileState, StateEntry, STATE_VERSION};
pub use manager::SessionStateManager;
