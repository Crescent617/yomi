//! File state tracking - track file modification times for stale read detection

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Version of the file state file format
pub const STATE_VERSION: u32 = 1;

/// File modification state for a single file
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileState {
    pub path: PathBuf,
    pub mtime: u64,
}

impl FileState {
    /// Create a new file state
    pub fn new(path: PathBuf, mtime: u64) -> Self {
        Self { path, mtime }
    }
}

/// A single entry in the state file (JSONL format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum StateEntry {
    /// Metadata header (first line)
    #[serde(rename = "meta")]
    Metadata { v: u32, created: String },

    /// File state entry
    #[serde(rename = "file")]
    FileState { p: PathBuf, m: u64 },
}

impl From<FileState> for StateEntry {
    fn from(fs: FileState) -> Self {
        StateEntry::FileState {
            p: fs.path,
            m: fs.mtime,
        }
    }
}

impl TryFrom<StateEntry> for FileState {
    type Error = &'static str;

    fn try_from(entry: StateEntry) -> std::result::Result<Self, Self::Error> {
        match entry {
            StateEntry::FileState { p, m } => Ok(FileState { path: p, mtime: m }),
            StateEntry::Metadata { .. } => Err("not a file state entry"),
        }
    }
}

use crate::types::{KernelError, Result};
use async_trait::async_trait;
use std::collections::HashMap;

/// Storage for file modification states
#[async_trait]
pub trait FileStateStore: Send + Sync {
    /// Record a file state
    async fn record(&self, path: PathBuf, mtime: u64) -> Result<()>;

    /// Get all recorded file states (latest entry per path wins)
    async fn get_all(&self) -> Result<HashMap<PathBuf, u64>>;

    /// Clear all file states
    async fn clear(&self) -> Result<()>;
}

/// Helper for storage errors
fn storage_err(msg: impl Into<String>) -> KernelError {
    KernelError::Storage(msg.into())
}

pub mod jsonl;
pub use jsonl::JsonlFileStateStore;
